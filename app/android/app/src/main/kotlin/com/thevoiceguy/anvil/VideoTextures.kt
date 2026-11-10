package com.thevoiceguy.anvil

import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import io.flutter.view.TextureRegistry

/**
 * Textures for video tiles (app/rust/src/api/video.rs), as on the other
 * platforms: pictures never cross into Dart. `create` makes a texture with
 * a surface Flutter draws from, and a slot in the Rust library for that
 * surface; it answers the texture's id, the address of the Rust function
 * that draws on a slot's surface, and the slot. Dart hands the last two to
 * Rust, whose video threads draw each picture, and detaches it from Rust
 * before it asks to `dispose` of the texture, so the slot is freed after
 * its last picture.
 */
object VideoTextures {
    private class Tile(val producer: TextureRegistry.SurfaceProducer, val slot: Long)

    private val tiles = HashMap<Long, Tile>()

    fun register(engine: FlutterEngine) {
        val registry = engine.renderer
        val channel = MethodChannel(engine.dartExecutor.binaryMessenger, "anvil/video")
        channel.setMethodCallHandler { call, result ->
            when (call.method) {
                "create" -> {
                    val producer = registry.createSurfaceProducer()
                    // A first size; each picture sets its own.
                    producer.setSize(640, 360)
                    val slot = AnvilNative.videoSlot()
                    // Flutter lets the surface go while the app is away and
                    // makes another on its return.
                    producer.setCallback(
                        object : TextureRegistry.SurfaceProducer.Callback {
                            override fun onSurfaceAvailable() =
                                AnvilNative.videoSurface(slot, producer.surface)

                            override fun onSurfaceCleanup() = AnvilNative.videoSurface(slot, null)
                        },
                    )
                    AnvilNative.videoSurface(slot, producer.surface)
                    tiles[producer.id()] = Tile(producer, slot)
                    result.success(
                        mapOf(
                            "texture" to producer.id(),
                            "present" to AnvilNative.videoPresent(),
                            "handle" to slot,
                        ),
                    )
                }
                "dispose" -> {
                    val id = call.argument<Number>("texture")?.toLong()
                    tiles.remove(id)?.let {
                        AnvilNative.videoSurface(it.slot, null)
                        AnvilNative.videoRelease(it.slot)
                        it.producer.release()
                    }
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
    }

    /** The engine is going: its textures go with it. */
    fun clear() {
        tiles.values.forEach {
            AnvilNative.videoSurface(it.slot, null)
            it.producer.release()
        }
        // The slots stay: Rust may still hold them until Dart detached,
        // and a slot without a surface draws nothing.
        tiles.clear()
    }
}
