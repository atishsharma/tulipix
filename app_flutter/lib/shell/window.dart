// The one thing the framework cannot do: the toplevel window.
//
// Flutter 3.47 has no public API for OS fullscreen on desktop, and the zen
// player is real fullscreen in the Slint build -- `set_fullscreen(true)` on
// enter, restored on exit. The runner in linux/runner/my_application.cc
// answers one method on one channel; taking a window-management package for a
// single boolean would be the larger change.

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'tray_panel.dart';

const MethodChannel _window = MethodChannel('tulipix/window');

/// Raise and focus the window.
///
/// What the tray's Open item and MPRIS `Raise` mean: the app may be behind
/// everything, or minimised, and a media applet that says "open the player"
/// has to actually open it.
Future<void> presentWindow() => _call('present');

/// Close the window, which ends the app. MPRIS `Quit` and the tray's Quit item.
Future<void> closeWindow() => _call('close');

Future<void> _call(String method, [Object? arg]) async {
  try {
    await _window.invokeMethod<void>(method, arg);
  } on PlatformException {
    // No handler on this platform.
  } on MissingPluginException {
    // Same, before the engine has a channel at all.
  }
}

/// Ask the window manager for fullscreen. A no-op wherever the channel is not
/// answered -- there is no other platform yet, and a missing handler is not an
/// error worth surfacing to someone who pressed a play button.
Future<void> setWindowFullscreen(bool on) => _call('setFullscreen', on);

/// Name the window after what is playing, so the taskbar entry and the alt-tab
/// card say it too.
///
/// `MaterialApp.title` is not this: on desktop Linux it names nothing the
/// window manager reads. The toplevel's title is the runner's to set, and on
/// GNOME the header bar carries its own copy that has to be set with it --
/// which is why this is one method rather than one `gtk_window_set_title`.
Future<void> setWindowTitle(String title) => _call('setTitle', title);

/// Ask the compositor to move the window, because a frameless one cannot move
/// itself — Wayland has no window-positioning API at all.
Future<void> beginWindowDrag() => _call('beginDrag');

/// Ask it to resize from one edge. [edge] is a `GdkWindowEdge`, which is what
/// [WindowEdge] is ordered by.
Future<void> beginWindowResize(WindowEdge edge) =>
    _call('beginResize', edge.index);

/// Turn the window INTO the mini widget, or give it back.
///
/// `open_mini` in crates/tulipix-app/src/miniwin.rs shows a second frameless
/// toplevel and hides the main one — "the app becomes the widget". Flutter's
/// desktop embedding opens exactly one window, so the one window becomes it:
/// shrunk to [size], kept above other windows, and painting on nothing so the
/// widget's ring and its shadow have somewhere to fall. Leaving puts back the
/// geometry the window had, maximised state included.
Future<void> setWindowWidgetMode(bool on, Size size) => _call('widgetMode', {
      'on': on,
      'width': size.width.round(),
      'height': size.height.round(),
    });

/// Resize the window while it IS the widget — the scale grip. Ignored by the
/// runner at any other time: the window's size belongs to the user then.
Future<void> setWindowSize(Size size) => _call('setSize', {
      'width': size.width.round(),
      'height': size.height.round(),
    });

/// Open or drop the tray panel's own toplevel: a second view on this engine,
/// drawn by this isolate. See lib/shell/tray_panel.dart for the order the two
/// halves have to happen in.
Future<void> setPanelWindow(bool on) => _call('panel', on);

/// Cut the panel's window to the height its content came out at.
Future<void> setPanelHeight(int height) => _call('panelHeight', height);

/// The widget's pin: keep it above other windows, or let it go behind them.
/// The runner remembers it and applies it whenever the widget opens.
Future<void> setWindowKeepAbove(bool on) => _call('keepAbove', on);

Future<void> minimizeWindow() => _call('minimize');
Future<void> toggleMaximizeWindow() => _call('toggleMaximize');

/// The eight handles, in `GdkWindowEdge` order. The runner passes the index
/// straight through to `gtk_window_begin_resize_drag`, so this order is the
/// contract rather than a preference.
enum WindowEdge {
  northWest,
  north,
  northEast,
  west,
  east,
  southWest,
  south,
  southEast;

  SystemMouseCursor get cursor => switch (this) {
        WindowEdge.northWest ||
        WindowEdge.southEast =>
          SystemMouseCursors.resizeUpLeftDownRight,
        WindowEdge.northEast ||
        WindowEdge.southWest =>
          SystemMouseCursors.resizeUpRightDownLeft,
        WindowEdge.north || WindowEdge.south => SystemMouseCursors.resizeUpDown,
        WindowEdge.west ||
        WindowEdge.east =>
          SystemMouseCursors.resizeLeftRight,
      };
}

/// Whether we are drawing the window's chrome, and what state it is in.
///
/// One object rather than a pair of futures: the caption row and the resize
/// edges both need the answer, and the maximised half changes behind the app's
/// back whenever a tiling shortcut or a snap-to-edge does it.
class WindowChrome extends ChangeNotifier {
  static final WindowChrome instance = WindowChrome._();

  WindowChrome._();

  /// True once the runner has said it dropped the system frame.
  ///
  /// The Linux runner always drops it — there is no switch any more — so the
  /// default there is true rather than false. That is not optimism: a frameless
  /// window whose app decided not to draw a caption row has no close button at
  /// all, and the only way to reach that state is the channel failing on the
  /// one platform where it cannot. Everywhere else the runner keeps the system
  /// frame and drawing caption buttons beside the OS's own would be worse than
  /// drawing none.
  bool custom = defaultTargetPlatform == TargetPlatform.linux;
  bool maximized = false;

  /// Whether "keep above" means anything here: X11 and XWayland yes, native
  /// Wayland no -- xdg-shell has no stacking request. The widget's pin is only
  /// drawn where it would do something.
  bool stacking = false;

  Future<void> init() async {
    _window.setMethodCallHandler((call) async {
      // Clicked past or Escaped: the runner asks rather than closing it
      // itself, because the view has to leave the tree first.
      if (call.method == 'onPanelDismiss') TrayPanel.instance.close();
      if (call.method == 'onMaximized' && call.arguments is bool) {
        maximized = call.arguments as bool;
        notifyListeners();
      }
      return null;
    });
    try {
      final info = await _window.invokeMapMethod<String, Object?>('chrome');
      if (info == null) return;
      custom = info['custom'] == true;
      maximized = info['maximized'] == true;
      stacking = info['stacking'] == true;
      notifyListeners();
    } on PlatformException {
      // Handler present but unhappy — keep the system frame.
    } on MissingPluginException {
      // No handler on this platform at all.
    }
  }
}
