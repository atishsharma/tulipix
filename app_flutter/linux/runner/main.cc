#include "my_application.h"

#include <ctype.h>

// Settings › Advanced › Display system, applied before GTK picks one.
//
// The backend is fixed the moment GDK opens its display, which is before any
// Dart runs and before the bridge is loaded -- so the runner reads the one key
// straight out of settings.json itself. A regex rather than a JSON parser:
// GLib has none, the key is ours and unique, and serde writes it as
// `"key": "value"`. A file that does not say is Automatic.
//
// The path is `tulipix_core::paths::settings_path()`: the user config dir,
// then "Tulipix" plus the profile's slug (`slugify_profile`), then the file.
static gchar* settings_path() {
  GString* dir = g_string_new("Tulipix");
  const gchar* profile = g_getenv("TULIPIX_PROFILE");
  if (profile != nullptr) {
    // `slugify_profile`: letters and digits lowercased, every other run one
    // dash, no dash at either end, forty characters at most.
    GString* slug = g_string_new(nullptr);
    gboolean dash = TRUE;
    for (const gchar* c = profile; *c != '\0'; c++) {
      if (isalnum(static_cast<unsigned char>(*c)) &&
          static_cast<unsigned char>(*c) < 128) {
        g_string_append_c(slug, tolower(static_cast<unsigned char>(*c)));
        dash = FALSE;
      } else if (!dash) {
        g_string_append_c(slug, '-');
        dash = TRUE;
      }
    }
    while (slug->len > 0 && slug->str[slug->len - 1] == '-') {
      g_string_truncate(slug, slug->len - 1);
    }
    if (slug->len > 40) {
      g_string_truncate(slug, 40);
    }
    if (slug->len > 0) {
      g_string_append_printf(dir, "-%s", slug->str);
    }
    g_string_free(slug, TRUE);
  }
  gchar* path = g_build_filename(g_get_user_config_dir(), dir->str,
                                 "settings.json", nullptr);
  g_string_free(dir, TRUE);
  return path;
}

static void apply_display_backend() {
  // Set by hand, it wins: that is how the choice was tried before it was a
  // setting, and a launcher that sets it means it.
  if (g_getenv("GDK_BACKEND") != nullptr) {
    return;
  }
  g_autofree gchar* path = settings_path();
  g_autofree gchar* text = nullptr;
  if (!g_file_get_contents(path, &text, nullptr, nullptr)) {
    return;
  }
  g_autoptr(GRegex) re = g_regex_new(
      "\"ui\\.display-backend\"\\s*:\\s*\"([^\"]*)\"", G_REGEX_CASELESS,
      static_cast<GRegexMatchFlags>(0), nullptr);
  g_autoptr(GMatchInfo) match = nullptr;
  if (re == nullptr || !g_regex_match(re, text, static_cast<GRegexMatchFlags>(0),
                                      &match)) {
    return;
  }
  g_autofree gchar* value = g_match_info_fetch(match, 1);
  // Each with the other as a fallback: a Wayland pick in a plain X11 session,
  // or an X11 pick where XWayland is not installed, still opens a window.
  if (g_ascii_strcasecmp(value, "X11") == 0) {
    g_setenv("GDK_BACKEND", "x11,wayland", TRUE);
  } else if (g_ascii_strcasecmp(value, "Wayland") == 0) {
    g_setenv("GDK_BACKEND", "wayland,x11", TRUE);
  }
}

int main(int argc, char** argv) {
  apply_display_backend();
  g_autoptr(MyApplication) app = my_application_new();
  return g_application_run(G_APPLICATION(app), argc, argv);
}
