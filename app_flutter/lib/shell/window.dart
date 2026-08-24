// The one thing the framework cannot do: the toplevel window.
//
// Flutter 3.47 has no public API for OS fullscreen on desktop, and the zen
// player is real fullscreen in the Slint build -- `set_fullscreen(true)` on
// enter, restored on exit. The runner in linux/runner/my_application.cc
// answers one method on one channel; taking a window-management package for a
// single boolean would be the larger change.

import 'package:flutter/services.dart';

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
