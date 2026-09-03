#include "my_application.h"

#include <flutter_linux/flutter_linux.h>

#include "flutter/generated_plugin_registrant.h"

struct _MyApplication {
  GtkApplication parent_instance;
  char** dart_entrypoint_arguments;
};

G_DEFINE_TYPE(MyApplication, my_application, GTK_TYPE_APPLICATION)

// Called when first Flutter frame received.
static void first_frame_cb(MyApplication* self, FlView* view) {
  gtk_widget_show(gtk_widget_get_toplevel(GTK_WIDGET(view)));
}

// The one thing Dart cannot reach on its own: the toplevel window.
//
// The zen player is real fullscreen in the Slint build --
// `w.window().set_fullscreen(true)` on enter, restored on exit -- and Flutter
// 3.47 has no framework API for it. Rather than take a window-management
// package for one boolean, this is a method channel with a single method.
static GtkWindow* g_main_window = nullptr;
static FlMethodChannel* g_window_channel = nullptr;

// What the window was before it became the widget, so leaving puts it back.
// Slint keeps two windows and hides one; there is only one here, so the
// geometry has to be remembered rather than left standing in a hidden toplevel.
static gint g_saved_w = 0;
static gint g_saved_h = 0;
static gboolean g_saved_max = FALSE;
static gboolean g_widget_mode = FALSE;

// A size the compositor would not take, and whether we have asked for
// fullscreen. See `resize_when_free` for what the pair is for.
static gint g_pending_w = 0;
static gint g_pending_h = 0;
static gboolean g_fullscreen = FALSE;

static gboolean map_bool(FlValue* map, const gchar* key) {
  FlValue* v = fl_value_lookup_string(map, key);
  return v != nullptr && fl_value_get_type(v) == FL_VALUE_TYPE_BOOL &&
         fl_value_get_bool(v);
}

static gint map_int(FlValue* map, const gchar* key, gint fallback) {
  FlValue* v = fl_value_lookup_string(map, key);
  if (v == nullptr || fl_value_get_type(v) != FL_VALUE_TYPE_INT) {
    return fallback;
  }
  return static_cast<gint>(fl_value_get_int(v));
}

// Whether a resize request would be honoured right now.
//
// A maximised or fullscreen window's geometry belongs to the compositor:
// `gtk_window_resize` on one is dropped on the floor, silently. Both sizes this
// runner asks for are asked at exactly the moment such a state is being left --
// the widget's box on the way out of maximised, the app's own box on the way
// out of zen's fullscreen -- so the request has to survive the state change
// rather than race it. Our own fullscreen intent counts even before the
// compositor has confirmed it, because on Wayland it confirms a frame later and
// the request that has to wait is already on its way.
static gboolean geometry_is_ours(GtkWindow* window) {
  if (g_fullscreen) {
    return FALSE;
  }
  GdkWindow* gdk = gtk_widget_get_window(GTK_WIDGET(window));
  if (gdk == nullptr) {
    return TRUE;
  }
  GdkWindowState state = gdk_window_get_state(gdk);
  return (state & (GDK_WINDOW_STATE_MAXIMIZED | GDK_WINDOW_STATE_FULLSCREEN)) ==
         0;
}

// Resize now, or as soon as the window's geometry is the app's to set again.
//
// Without this, opening the widget from a maximised window left the toplevel
// the size of the screen with the widget painted at 448x80 in the corner of it
// -- "Timed out waiting for OpenGL frame of size 1920x1040 (have 448x80)",
// which is the embedder saying exactly that.
static void resize_when_free(GtkWindow* window, gint width, gint height) {
  if (width <= 0 || height <= 0) {
    return;
  }
  if (geometry_is_ours(window)) {
    g_pending_w = 0;
    g_pending_h = 0;
    gtk_window_resize(window, width, height);
    return;
  }
  g_pending_w = width;
  g_pending_h = height;
}

// The window is always frameless: the app draws the only title bar there is.
//
// This used to be switchable with TULIPIX_NATIVE_FRAME, and the switch is gone
// rather than defaulted — a second title bar under GTK's own is not a fallback
// anybody wants, and the caption row carries the mini widget button, which the
// system frame has nowhere to put.
static gboolean chrome_is_custom() {
  return TRUE;
}

// Where the pointer is, in root coordinates.
//
// Both begin_move_drag and begin_resize_drag want the press that started the
// gesture, and by the time Dart has sent us a method call there is no current
// event left to read it off -- the engine has already handled and dropped it.
// Asking the seat where the pointer is now is what every Flutter windowing
// plugin does here, and it is right for the case that matters: the button is
// still down.
static void pointer_root_position(GtkWindow* window, gint* x, gint* y) {
  *x = 0;
  *y = 0;
  GdkDisplay* display = gtk_widget_get_display(GTK_WIDGET(window));
  if (display == nullptr) {
    return;
  }
  GdkSeat* seat = gdk_display_get_default_seat(display);
  if (seat == nullptr) {
    return;
  }
  GdkDevice* pointer = gdk_seat_get_pointer(seat);
  if (pointer == nullptr) {
    return;
  }
  gdk_device_get_position(pointer, nullptr, x, y);
}

// Tell Dart the window was maximised or restored, including when the WM did it
// (a tiling shortcut, a double-click on nothing, a snap to the top edge). The
// caption row draws a different glyph for the two states and would otherwise be
// telling the user something false.
static gboolean window_state_cb(GtkWidget* widget, GdkEventWindowState* event,
                                gpointer user_data) {
  if ((event->changed_mask & GDK_WINDOW_STATE_MAXIMIZED) != 0 &&
      g_window_channel != nullptr) {
    g_autoptr(FlValue) on = fl_value_new_bool(
        (event->new_window_state & GDK_WINDOW_STATE_MAXIMIZED) != 0);
    fl_method_channel_invoke_method(g_window_channel, "onMaximized", on,
                                    nullptr, nullptr, nullptr);
  }
  // The size a resize request could not have while the compositor owned the
  // geometry. This is the half of `resize_when_free` that lands it: leaving
  // zen puts the app back at the size it had rather than at the widget's box,
  // and opening the widget from a maximised window gets the widget's box.
  if (g_pending_w > 0 && g_pending_h > 0 && g_main_window != nullptr &&
      !g_fullscreen &&
      (event->new_window_state & (GDK_WINDOW_STATE_MAXIMIZED |
                                  GDK_WINDOW_STATE_FULLSCREEN)) == 0) {
    gint w = g_pending_w;
    gint h = g_pending_h;
    g_pending_w = 0;
    g_pending_h = 0;
    gtk_window_resize(g_main_window, w, h);
  }
  return FALSE;
}

static void window_method_call_cb(FlMethodChannel* channel,
                                  FlMethodCall* method_call,
                                  gpointer user_data) {
  g_autoptr(FlMethodResponse) response = nullptr;
  const gchar* name = fl_method_call_get_name(method_call);
  if (g_strcmp0(name, "setFullscreen") == 0) {
    FlValue* args = fl_method_call_get_args(method_call);
    gboolean on = fl_value_get_type(args) == FL_VALUE_TYPE_BOOL &&
                  fl_value_get_bool(args);
    if (g_main_window != nullptr) {
      g_fullscreen = on;
      if (on) {
        gtk_window_fullscreen(g_main_window);
      } else {
        gtk_window_unfullscreen(g_main_window);
      }
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "present") == 0) {
    // The tray's Open item and MPRIS `Raise`. Deiconify first: presenting a
    // minimised window on most WMs restores it, but not all of them, and a
    // "show me the player" that leaves it in the taskbar is a bug report.
    if (g_main_window != nullptr) {
      gtk_window_deiconify(g_main_window);
      gtk_window_present(g_main_window);
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "chrome") == 0) {
    // Asked once at startup: whether Dart has to draw the caption row and the
    // resize edges, and whether the window is maximised right now.
    g_autoptr(FlValue) info = fl_value_new_map();
    fl_value_set_string_take(info, "custom",
                             fl_value_new_bool(chrome_is_custom()));
    fl_value_set_string_take(
        info, "maximized",
        fl_value_new_bool(g_main_window != nullptr &&
                          gtk_window_is_maximized(g_main_window)));
    response = FL_METHOD_RESPONSE(fl_method_success_response_new(info));
  } else if (g_strcmp0(name, "beginDrag") == 0) {
    // Wayland has no window-positioning API: a frameless window cannot move
    // itself, it can only hand the move to the compositor. Same call covers
    // X11, so there is one path rather than two.
    if (g_main_window != nullptr) {
      gint x = 0;
      gint y = 0;
      pointer_root_position(g_main_window, &x, &y);
      guint32 when = gtk_get_current_event_time();
      // ponytail: temporary. "Dragging the caption row does nothing" has two
      // very different causes -- the press never reaching Dart's hit test, or
      // the compositor refusing the move -- and this line says which. Delete
      // it once that is known.
      g_message("beginDrag: pointer=%d,%d time=%u", x, y, when);
      gtk_window_begin_move_drag(g_main_window, 1, x, y, when);
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "beginResize") == 0) {
    // An undecorated window has no resize border of its own; the eight handles
    // Dart draws round the edge send the edge they represent. The int is a
    // GdkWindowEdge, which is why Dart's enum is in that order and not a nicer
    // one.
    FlValue* args = fl_method_call_get_args(method_call);
    if (g_main_window != nullptr && args != nullptr &&
        fl_value_get_type(args) == FL_VALUE_TYPE_INT) {
      int64_t edge = fl_value_get_int(args);
      if (edge >= GDK_WINDOW_EDGE_NORTH_WEST &&
          edge <= GDK_WINDOW_EDGE_SOUTH_EAST) {
        gint x = 0;
        gint y = 0;
        pointer_root_position(g_main_window, &x, &y);
        gtk_window_begin_resize_drag(g_main_window,
                                     static_cast<GdkWindowEdge>(edge), 1, x, y,
                                     gtk_get_current_event_time());
      }
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "minimize") == 0) {
    if (g_main_window != nullptr) {
      gtk_window_iconify(g_main_window);
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "toggleMaximize") == 0) {
    // Never while the window IS the widget. A maximised widget is a 448x80
    // card in the corner of a full-screen toplevel, and the only way back out
    // of it is the caption row the widget does not have.
    if (g_main_window != nullptr && !g_widget_mode) {
      if (gtk_window_is_maximized(g_main_window)) {
        gtk_window_unmaximize(g_main_window);
      } else {
        gtk_window_maximize(g_main_window);
      }
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "widgetMode") == 0) {
    // "Show the widget and hide the main window -- the app becomes the widget",
    // which is `open_mini` in crates/tulipix-app/src/miniwin.rs. Slint has two
    // toplevels and hides one; Flutter's desktop embedding opens exactly one
    // window, so the one window IS the widget for as long as it is up: shrunk
    // to the widget's box, above the other windows, painting on nothing.
    FlValue* args = fl_method_call_get_args(method_call);
    if (g_main_window != nullptr && args != nullptr &&
        fl_value_get_type(args) == FL_VALUE_TYPE_MAP) {
      gboolean on = map_bool(args, "on");
      if (on != g_widget_mode) {
        g_widget_mode = on;
        if (on) {
          // Remembered BEFORE anything moves, and a maximised window has to be
          // un-maximised first: a resize request on one is dropped, so the
          // widget would have come up the size of the whole screen.
          gtk_window_get_size(g_main_window, &g_saved_w, &g_saved_h);
          g_saved_max = gtk_window_is_maximized(g_main_window);
          if (g_saved_max) {
            gtk_window_unmaximize(g_main_window);
          }
          // The pin the Slint widget offers, minus the button: on Wayland
          // there is no stacking request to make and this is a no-op, which is
          // the same reason `no-stacking-control` drops the pin there.
          gtk_window_set_keep_above(g_main_window, TRUE);
          resize_when_free(g_main_window, map_int(args, "width", 441),
                           map_int(args, "height", 212));
        } else {
          gtk_window_set_keep_above(g_main_window, FALSE);
          // Before the maximise, not after: it is the size the window comes
          // back to when that maximise is undone, and a widget-sized restore
          // is the bug this order avoids.
          resize_when_free(g_main_window, g_saved_w, g_saved_h);
          if (g_saved_max) {
            gtk_window_maximize(g_main_window);
          }
        }
      }
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "setSize") == 0) {
    // Only while the window IS the widget. Everywhere else the window's size
    // belongs to the user and the compositor, and a stray resize from the app
    // would be the app arguing with them.
    FlValue* args = fl_method_call_get_args(method_call);
    if (g_main_window != nullptr && g_widget_mode && args != nullptr &&
        fl_value_get_type(args) == FL_VALUE_TYPE_MAP) {
      resize_when_free(g_main_window, map_int(args, "width", 441),
                       map_int(args, "height", 212));
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "setTitle") == 0) {
    // What is playing, in the taskbar and the alt-tab card. The app draws the
    // title row inside the window; this is the copy the shell reads.
    FlValue* args = fl_method_call_get_args(method_call);
    if (g_main_window != nullptr && args != nullptr &&
        fl_value_get_type(args) == FL_VALUE_TYPE_STRING) {
      gtk_window_set_title(g_main_window, fl_value_get_string(args));
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else if (g_strcmp0(name, "close") == 0) {
    if (g_main_window != nullptr) {
      gtk_window_close(g_main_window);
    }
    response = FL_METHOD_RESPONSE(
        fl_method_success_response_new(fl_value_new_null()));
  } else {
    response = FL_METHOD_RESPONSE(fl_method_not_implemented_response_new());
  }
  fl_method_call_respond(method_call, response, nullptr);
}

// The taskbar mark.
//
// Neither X11 nor Wayland carries an icon the app can simply push: the shell
// matches the toplevel's WM_CLASS / app_id against an installed `.desktop` and
// takes the icon from there. That works once Tulipix is installed and not
// before — so the icon is also loaded straight off disk, from beside the
// executable, which covers a run out of the build directory. `set_default`
// applies it to every window the process opens, present and future.
static void set_taskbar_icon() {
  gtk_window_set_default_icon_name(APPLICATION_WM_CLASS);
  g_autofree gchar* exe = g_file_read_link("/proc/self/exe", nullptr);
  if (exe == nullptr) {
    return;
  }
  g_autofree gchar* dir = g_path_get_dirname(exe);
  g_autofree gchar* icon = g_build_filename(dir, "data", "tulipix.png", nullptr);
  if (!g_file_test(icon, G_FILE_TEST_EXISTS)) {
    return;
  }
  g_autoptr(GError) error = nullptr;
  if (!gtk_window_set_default_icon_from_file(icon, &error)) {
    g_warning("taskbar icon: %s", error->message);
  }
}

// Implements GApplication::activate.
static void my_application_activate(GApplication* application) {
  MyApplication* self = MY_APPLICATION(application);
  set_taskbar_icon();

  GtkWindow* window =
      GTK_WINDOW(gtk_application_window_new(GTK_APPLICATION(application)));
  // WM_CLASS, which is what a shell matches against `StartupWMClass` in the
  // .desktop file. `g_set_prgname` below sets the instance half; this is the
  // class half, and GTK only takes it before the window is realized.
  gtk_window_set_role(window, APPLICATION_WM_CLASS);

  // An RGBA visual, so the widget mode below has real alpha to paint on. It
  // has to be set before the window is realized, and it costs the normal
  // window nothing: every pixel under the shell is painted opaque anyway.
  //
  // The view's background follows from it once, at creation, rather than being
  // switched when the widget opens: by then the view has handed its surface to
  // the compositor and the setter is the unreliable half of this.
  GdkScreen* visual_screen = gtk_widget_get_screen(GTK_WIDGET(window));
  GdkVisual* rgba = gdk_screen_get_rgba_visual(visual_screen);
  gboolean alpha = rgba != nullptr && gdk_screen_is_composited(visual_screen);
  if (alpha) {
    gtk_widget_set_visual(GTK_WIDGET(window), rgba);
  }

  // No frame at all: the app draws the caption row, and the eight resize
  // handles round its edge stand in for the border that goes with it. The
  // window still needs a title -- it is what the taskbar and alt-tab read, and
  // Dart keeps it pointed at whatever is playing.
  //
  // The GtkHeaderBar the Flutter template installs on GNOME is gone with the
  // frame: it was a second title bar under ours, and there is no arrangement in
  // which both are wanted.
  //
  // set_decorated alone is not enough: it is advice, and KWin on Wayland
  // ignores it. GTK3 only claims a window's decoration for itself once that
  // window owns a titlebar widget, and an undecorated window owns none -- so
  // the compositor takes the decoration and lands its frame on top of the row
  // Dart draws, which is the two title bars this looked like on Plasma. An
  // empty box is the claim; it has no children, so it costs no height and
  // paints nothing. Both calls have to come before the window is realized.
  GtkWidget* own_the_decoration = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 0);
  gtk_widget_show(own_the_decoration);
  gtk_window_set_titlebar(window, own_the_decoration);
  gtk_window_set_decorated(window, FALSE);
  gtk_window_set_title(window, "Tulipix");

  gtk_window_set_default_size(window, 1280, 720);

  g_autoptr(FlDartProject) project = fl_dart_project_new();
  fl_dart_project_set_dart_entrypoint_arguments(
      project, self->dart_entrypoint_arguments);

  FlView* view = fl_view_new(project);
  GdkRGBA background_color;
  // Transparent wherever the compositor can carry it: the widget draws a
  // gradient ring with rounded corners and a shadow under it, and both need to
  // fall on nothing rather than on a black square. `background: transparent` on
  // `MiniWidget` in ui/mini_widget.slint, and its comment applies here too —
  // compositors without alpha just show the panel colour. The shell paints
  // every pixel it occupies, so this changes nothing outside widget mode.
  gdk_rgba_parse(&background_color, alpha ? "#00000000" : "#000000");
  fl_view_set_background_color(view, &background_color);
  gtk_widget_show(GTK_WIDGET(view));
  gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(view));

  // Show the window when Flutter renders.
  // Requires the view to be realized so we can start rendering.
  g_signal_connect_swapped(view, "first-frame", G_CALLBACK(first_frame_cb),
                           self);
  gtk_widget_realize(GTK_WIDGET(view));

  fl_register_plugins(FL_PLUGIN_REGISTRY(view));

  g_main_window = window;
  g_signal_connect(window, "window-state-event", G_CALLBACK(window_state_cb),
                   nullptr);
  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  g_window_channel = fl_method_channel_new(
      fl_engine_get_binary_messenger(fl_view_get_engine(view)),
      "tulipix/window", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(g_window_channel,
                                            window_method_call_cb, nullptr,
                                            nullptr);

  gtk_widget_grab_focus(GTK_WIDGET(view));
}

// Implements GApplication::local_command_line.
static gboolean my_application_local_command_line(GApplication* application,
                                                  gchar*** arguments,
                                                  int* exit_status) {
  MyApplication* self = MY_APPLICATION(application);
  // Strip out the first argument as it is the binary name.
  self->dart_entrypoint_arguments = g_strdupv(*arguments + 1);

  g_autoptr(GError) error = nullptr;
  if (!g_application_register(application, nullptr, &error)) {
    g_warning("Failed to register: %s", error->message);
    *exit_status = 1;
    return TRUE;
  }

  g_application_activate(application);
  *exit_status = 0;

  return TRUE;
}

// Implements GApplication::startup.
static void my_application_startup(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application startup.

  G_APPLICATION_CLASS(my_application_parent_class)->startup(application);
}

// Implements GApplication::shutdown.
static void my_application_shutdown(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application shutdown.

  G_APPLICATION_CLASS(my_application_parent_class)->shutdown(application);
}

// Implements GObject::dispose.
static void my_application_dispose(GObject* object) {
  MyApplication* self = MY_APPLICATION(object);
  g_clear_pointer(&self->dart_entrypoint_arguments, g_strfreev);
  G_OBJECT_CLASS(my_application_parent_class)->dispose(object);
}

static void my_application_class_init(MyApplicationClass* klass) {
  G_APPLICATION_CLASS(klass)->activate = my_application_activate;
  G_APPLICATION_CLASS(klass)->local_command_line =
      my_application_local_command_line;
  G_APPLICATION_CLASS(klass)->startup = my_application_startup;
  G_APPLICATION_CLASS(klass)->shutdown = my_application_shutdown;
  G_OBJECT_CLASS(klass)->dispose = my_application_dispose;
}

static void my_application_init(MyApplication* self) {}

MyApplication* my_application_new() {
  // The program name is what GTK writes into WM_CLASS and what a shell matches
  // against `StartupWMClass` in the installed .desktop file — which is the only
  // route by which a running window gets an icon on X11 or Wayland. The default
  // here is the dotted application id, and nothing in packaging/linux declares
  // that, so the taskbar had a blank square.
  g_set_prgname(APPLICATION_WM_CLASS);

  return MY_APPLICATION(g_object_new(my_application_get_type(),
                                     "application-id", APPLICATION_ID, "flags",
                                     G_APPLICATION_NON_UNIQUE, nullptr));
}
