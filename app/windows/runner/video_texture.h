#ifndef RUNNER_VIDEO_TEXTURE_H_
#define RUNNER_VIDEO_TEXTURE_H_

#include <flutter/encodable_value.h>
#include <flutter/flutter_engine.h>
#include <flutter/method_channel.h>
#include <flutter/texture_registrar.h>

#include <cstdint>
#include <map>
#include <memory>

struct VideoTexture;

// Textures for video tiles, over `anvil/video` (app/rust/src/api/video.rs).
class VideoTextures {
 public:
  explicit VideoTextures(flutter::FlutterEngine* engine);
  ~VideoTextures();

 private:
  flutter::TextureRegistrar* registrar_;
  std::unique_ptr<flutter::MethodChannel<flutter::EncodableValue>> channel_;
  // The textures made, by id, until disposed of.
  std::map<int64_t, std::shared_ptr<VideoTexture>> textures_;
};

#endif  // RUNNER_VIDEO_TEXTURE_H_
