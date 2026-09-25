# media_kit_video 2.0.1, patched

Upstream: https://github.com/media-kit/media-kit, `media_kit_video` 2.0.1 from pub.dev, unchanged except:

- `linux/texture_gl.cc`: `mpv_render_context_render` is passed
  `MPV_RENDER_PARAM_BLOCK_FOR_TARGET_TIME = 0`.

Why: the render runs inside Flutter's texture callback on the raster thread.
With mpv's default (1) it waits for each frame's display time, up to
`video-timing-offset` (50 ms), every video frame. The app then renders at
whatever rate that leaves, and video stutters at any resolution. Standalone mpv
does the same wait on its own thread, which is why it is smooth. Flutter paces
to vsync itself, so the wait is not needed. Frames now reach the screen up to
a few tens of ms early, which is within what is noticeable for lip sync.

Remove the `dependency_overrides` entry in `app_flutter/pubspec.yaml` and this
folder once upstream makes the same change.
