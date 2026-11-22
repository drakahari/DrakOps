eframe 0.33.3 vendored from crates.io, licensed MIT OR Apache-2.0, for a private DrakOps candidate.
The Linux/Wayland-only nonblocking swap and frame pacing are a narrow backport of
https://github.com/emilk/egui/pull/8631 / commit 85de2e4a1c92f53a432fedf2662df8b66b6cd89c,
by Carmine Paolino with Emil Ernerfeldt and Lucas Meurer. The upstream 250 ms App::logic
watchdog is not backported; that API does not exist in eframe 0.33.3.
