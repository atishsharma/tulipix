#include "my_application.h"

#include <flutter_linux/flutter_linux.h>
#ifdef GDK_WINDOWING_X11
#include <gdk/gdkx.h>
#endif

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

  // Use a header bar when running in GNOME as this is the common style used
  // by applications and is the setup most users will be using (e.g. Ubuntu
  // desktop).
  // If running on X and not using GNOME then just use a traditional title bar
  // in case the window manager does more exotic layout, e.g. tiling.
  // If running on Wayland assume the header bar will work (may need changing
  // if future cases occur).
  gboolean use_header_bar = TRUE;
#ifdef GDK_WINDOWING_X11
  GdkScreen* screen = gtk_window_get_screen(window);
  if (GDK_IS_X11_SCREEN(screen)) {
    const gchar* wm_name = gdk_x11_screen_get_window_manager_name(screen);
    if (g_strcmp0(wm_name, "GNOME Shell") != 0) {
      use_header_bar = FALSE;
    }
  }
#endif
  if (use_header_bar) {
    GtkHeaderBar* header_bar = GTK_HEADER_BAR(gtk_header_bar_new());
    gtk_widget_show(GTK_WIDGET(header_bar));
    gtk_header_bar_set_title(header_bar, "Tulipix");
    gtk_header_bar_set_show_close_button(header_bar, TRUE);
    gtk_window_set_titlebar(window, GTK_WIDGET(header_bar));
  } else {
    gtk_window_set_title(window, "Tulipix");
  }

  gtk_window_set_default_size(window, 1280, 720);

  g_autoptr(FlDartProject) project = fl_dart_project_new();
  fl_dart_project_set_dart_entrypoint_arguments(
      project, self->dart_entrypoint_arguments);

  FlView* view = fl_view_new(project);
  GdkRGBA background_color;
  // Background defaults to black, override it here if necessary, e.g. #00000000
  // for transparent.
  gdk_rgba_parse(&background_color, "#000000");
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
