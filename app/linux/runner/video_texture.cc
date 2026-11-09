// Textures for video tiles (app/rust/src/api/video.rs): pictures never
// cross into Dart. Dart asks `anvil/video` to `create` a texture and gets
// its id, the address of `anvil_video_present` and the texture's own
// address; it hands the last two to Rust, whose video threads call
// `anvil_video_present` with each picture as RGBA. Dart detaches it from
// Rust before it asks to `dispose` of the texture, so the function is never
// called on a texture that has gone.

#include "video_texture.h"

#include <cstdint>
#include <cstring>
#include <utility>
#include <vector>

#define ANVIL_VIDEO_TEXTURE(obj)                                   \
  (G_TYPE_CHECK_INSTANCE_CAST((obj), anvil_video_texture_get_type(), \
                              AnvilVideoTexture))

struct AnvilVideoTexture {
  FlPixelBufferTexture parent_instance;
  FlTextureRegistrar* registrar;
  GMutex lock;
  // The picture last presented, and whether the screen has taken it; only
  // the render thread touches `shown`, swapping it with `pending`.
  std::vector<uint8_t>* pending;
  std::vector<uint8_t>* shown;
  uint32_t pending_width, pending_height;
  uint32_t shown_width, shown_height;
  bool fresh;
};

struct AnvilVideoTextureClass {
  FlPixelBufferTextureClass parent_class;
};

G_DEFINE_TYPE(AnvilVideoTexture, anvil_video_texture,
              fl_pixel_buffer_texture_get_type())

static const uint8_t kBlack[4] = {0, 0, 0, 255};

static gboolean anvil_video_texture_copy_pixels(FlPixelBufferTexture* texture,
                                                const uint8_t** out_buffer,
                                                uint32_t* width,
                                                uint32_t* height,
                                                GError** error) {
  AnvilVideoTexture* self = ANVIL_VIDEO_TEXTURE(texture);
  g_mutex_lock(&self->lock);
  if (self->fresh) {
    std::swap(self->pending, self->shown);
    self->shown_width = self->pending_width;
    self->shown_height = self->pending_height;
    self->fresh = false;
  }
  g_mutex_unlock(&self->lock);
  if (self->shown->empty()) {
    *out_buffer = kBlack;
    *width = 1;
    *height = 1;
  } else {
    *out_buffer = self->shown->data();
    *width = self->shown_width;
    *height = self->shown_height;
  }
  return TRUE;
}

static void anvil_video_texture_finalize(GObject* object) {
  AnvilVideoTexture* self = ANVIL_VIDEO_TEXTURE(object);
  delete self->pending;
  delete self->shown;
  g_mutex_clear(&self->lock);
  G_OBJECT_CLASS(anvil_video_texture_parent_class)->finalize(object);
}

static void anvil_video_texture_class_init(AnvilVideoTextureClass* klass) {
  FL_PIXEL_BUFFER_TEXTURE_CLASS(klass)->copy_pixels =
      anvil_video_texture_copy_pixels;
  G_OBJECT_CLASS(klass)->finalize = anvil_video_texture_finalize;
}

static void anvil_video_texture_init(AnvilVideoTexture* self) {
  g_mutex_init(&self->lock);
  self->pending = new std::vector<uint8_t>();
  self->shown = new std::vector<uint8_t>();
  self->pending_width = self->pending_height = 0;
  self->shown_width = self->shown_height = 0;
  self->fresh = false;
}

// Called by Rust on a video thread with a picture to show now.
extern "C" void anvil_video_present(void* texture, const uint8_t* rgba,
                                    uint32_t width, uint32_t height) {
  AnvilVideoTexture* self = static_cast<AnvilVideoTexture*>(texture);
  const size_t size = static_cast<size_t>(width) * height * 4;
  g_mutex_lock(&self->lock);
  self->pending->assign(rgba, rgba + size);
  self->pending_width = width;
  self->pending_height = height;
  self->fresh = true;
  g_mutex_unlock(&self->lock);
  // Thread-safe: the registrar guards its textures with a mutex.
  fl_texture_registrar_mark_texture_frame_available(self->registrar,
                                                    FL_TEXTURE(self));
}

struct VideoTextures {
  FlTextureRegistrar* registrar;
  FlMethodChannel* channel;
  // The textures made, by id: a reference each until disposed of.
  GHashTable* textures;
};

// A texture id as a hash key: ids are pointer-sized.
static gpointer key_of(int64_t id) {
  return reinterpret_cast<gpointer>(static_cast<intptr_t>(id));
}

static FlMethodResponse* create_texture(VideoTextures* v) {
  AnvilVideoTexture* texture = ANVIL_VIDEO_TEXTURE(
      g_object_new(anvil_video_texture_get_type(), nullptr));
  texture->registrar = v->registrar;
  if (!fl_texture_registrar_register_texture(v->registrar,
                                             FL_TEXTURE(texture))) {
    g_object_unref(texture);
    return FL_METHOD_RESPONSE(fl_method_error_response_new(
        "texture", "the texture was not registered", nullptr));
  }
  const int64_t id = fl_texture_get_id(FL_TEXTURE(texture));
  g_hash_table_insert(v->textures, key_of(id), texture);
  g_autoptr(FlValue) result = fl_value_new_map();
  fl_value_set_string_take(result, "texture", fl_value_new_int(id));
  fl_value_set_string_take(
      result, "present",
      fl_value_new_int(reinterpret_cast<int64_t>(&anvil_video_present)));
  fl_value_set_string_take(
      result, "handle", fl_value_new_int(reinterpret_cast<int64_t>(texture)));
  return FL_METHOD_RESPONSE(fl_method_success_response_new(result));
}

static FlMethodResponse* dispose_texture(VideoTextures* v, FlValue* args) {
  FlValue* id = fl_value_get_type(args) == FL_VALUE_TYPE_MAP
                    ? fl_value_lookup_string(args, "texture")
                    : nullptr;
  if (id == nullptr || fl_value_get_type(id) != FL_VALUE_TYPE_INT) {
    return FL_METHOD_RESPONSE(
        fl_method_error_response_new("texture", "no texture id", nullptr));
  }
  gpointer key = key_of(fl_value_get_int(id));
  auto* texture =
      static_cast<AnvilVideoTexture*>(g_hash_table_lookup(v->textures, key));
  if (texture != nullptr) {
    fl_texture_registrar_unregister_texture(v->registrar, FL_TEXTURE(texture));
    g_hash_table_remove(v->textures, key);
  }
  return FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
}

static void method_call_cb(FlMethodChannel* channel, FlMethodCall* call,
                           gpointer user_data) {
  auto* v = static_cast<VideoTextures*>(user_data);
  const gchar* method = fl_method_call_get_name(call);
  g_autoptr(FlMethodResponse) response = nullptr;
  if (strcmp(method, "create") == 0) {
    response = create_texture(v);
  } else if (strcmp(method, "dispose") == 0) {
    response = dispose_texture(v, fl_method_call_get_args(call));
  } else {
    response = FL_METHOD_RESPONSE(fl_method_not_implemented_response_new());
  }
  fl_method_call_respond(call, response, nullptr);
}

void video_texture_register(FlPluginRegistry* registry) {
  g_autoptr(FlPluginRegistrar) registrar =
      fl_plugin_registry_get_registrar_for_plugin(registry, "AnvilVideo");
  // Lives as long as the app.
  auto* v = new VideoTextures();
  v->registrar = fl_plugin_registrar_get_texture_registrar(registrar);
  v->textures = g_hash_table_new_full(g_direct_hash, g_direct_equal, nullptr,
                                      g_object_unref);
  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  v->channel = fl_method_channel_new(
      fl_plugin_registrar_get_messenger(registrar), "anvil/video",
      FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(v->channel, method_call_cb, v,
                                            nullptr);
}
