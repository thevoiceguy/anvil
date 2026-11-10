import CoreVideo
import Flutter

/// Textures for video tiles (app/rust/src/api/video.rs), as on macOS
/// (macos/Runner/MainFlutterWindow.swift): pictures never cross into Dart.
/// `create` makes a texture and answers its id, the address of `present`
/// and the texture's own address; Dart hands the last two to Rust, whose
/// video threads call `present` with each picture as BGRA. Dart detaches it
/// from Rust before it asks to `dispose` of the texture, so `present` is
/// never called on a texture that has gone.
enum VideoTextures {
  static var textures: [Int64: VideoTexture] = [:]

  static func register(with registrar: FlutterPluginRegistrar) {
    let registry = registrar.textures()
    let channel = FlutterMethodChannel(name: "anvil/video", binaryMessenger: registrar.messenger())
    channel.setMethodCallHandler { call, result in
      switch call.method {
      case "create":
        let texture = VideoTexture(registry: registry)
        let id = registry.register(texture)
        texture.id = id
        textures[id] = texture
        let present: @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<UInt8>?, UInt32, UInt32) -> Void = videoPresent
        result([
          "texture": id,
          "present": Int64(Int(bitPattern: unsafeBitCast(present, to: UnsafeRawPointer.self))),
          "handle": Int64(Int(bitPattern: Unmanaged.passUnretained(texture).toOpaque())),
        ])
      case "dispose":
        if let id = (call.arguments as? [String: Any])?["texture"] as? Int64 {
          registry.unregisterTexture(id)
          textures.removeValue(forKey: id)
        }
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }
}

/// Called by Rust on a video thread with a picture to show now.
private func videoPresent(
  _ texture: UnsafeMutableRawPointer?, _ bgra: UnsafePointer<UInt8>?, _ width: UInt32, _ height: UInt32
) {
  guard let texture = texture, let bgra = bgra else { return }
  Unmanaged<VideoTexture>.fromOpaque(texture).takeUnretainedValue()
    .present(bgra, width: Int(width), height: Int(height))
}

/// One tile's texture: the last picture, as a pixel buffer Flutter copies.
final class VideoTexture: NSObject, FlutterTexture {
  let registry: FlutterTextureRegistry
  var id: Int64 = 0
  private let lock = NSLock()
  private var latest: CVPixelBuffer?

  init(registry: FlutterTextureRegistry) {
    self.registry = registry
  }

  func copyPixelBuffer() -> Unmanaged<CVPixelBuffer>? {
    lock.lock()
    defer { lock.unlock() }
    guard let buffer = latest else { return nil }
    return Unmanaged.passRetained(buffer)
  }

  func present(_ bgra: UnsafePointer<UInt8>, width: Int, height: Int) {
    let attributes: [String: Any] = [
      kCVPixelBufferMetalCompatibilityKey as String: true,
      kCVPixelBufferIOSurfacePropertiesKey as String: [String: Any](),
    ]
    var made: CVPixelBuffer?
    guard CVPixelBufferCreate(
      kCFAllocatorDefault, width, height, kCVPixelFormatType_32BGRA, attributes as CFDictionary, &made
    ) == kCVReturnSuccess, let buffer = made else { return }
    CVPixelBufferLockBaseAddress(buffer, [])
    if let base = CVPixelBufferGetBaseAddress(buffer) {
      let stride = CVPixelBufferGetBytesPerRow(buffer)
      let destination = base.assumingMemoryBound(to: UInt8.self)
      for row in 0..<height {
        memcpy(destination + row * stride, bgra + row * width * 4, width * 4)
      }
    }
    CVPixelBufferUnlockBaseAddress(buffer, [])
    lock.lock()
    latest = buffer
    lock.unlock()
    // Safe from any thread.
    registry.textureFrameAvailable(id)
  }
}
