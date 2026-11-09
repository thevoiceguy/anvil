#ifndef RUNNER_VIDEO_TEXTURE_H_
#define RUNNER_VIDEO_TEXTURE_H_

#include <flutter_linux/flutter_linux.h>

// Textures for video tiles, over `anvil/video` (app/rust/src/api/video.rs).
void video_texture_register(FlPluginRegistry* registry);

#endif  // RUNNER_VIDEO_TEXTURE_H_
