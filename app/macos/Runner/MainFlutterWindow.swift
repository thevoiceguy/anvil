import Cocoa
import CoreVideo
import FlutterMacOS
import ServiceManagement

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    StartAtLogin.register(with: flutterViewController.engine.binaryMessenger)
    Dock.register(with: flutterViewController.engine.binaryMessenger)
    Idle.register(with: flutterViewController.engine.binaryMessenger)
    VideoTextures.register(with: flutterViewController.registrar(forPlugin: "AnvilVideo"))

    super.awakeFromNib()
  }
}

/// Textures for video tiles (app/rust/src/api/video.rs): pictures never
/// cross into Dart. `create` makes a texture and answers its id, the address
/// of `present` and the texture's own address; Dart hands the last two to
/// Rust, whose video threads call `present` with each picture as BGRA. Dart
/// detaches it from Rust before it asks to `dispose` of the texture, so
/// `present` is never called on a texture that has gone.
enum VideoTextures {
  static var textures: [Int64: VideoTexture] = [:]

  static func register(with registrar: FlutterPluginRegistrar) {
    let registry = registrar.textures
    let channel = FlutterMethodChannel(name: "anvil/video", binaryMessenger: registrar.messenger)
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

/// The Dock's icon while the app runs: a brand's PNG, or the bundle's own
/// again with none (lib/src/desktop/native_shell.dart).
enum Dock {
  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "anvil/dock", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      guard call.method == "setIcon" else {
        result(FlutterMethodNotImplemented)
        return
      }
      if let path = call.arguments as? String, let image = NSImage(contentsOfFile: path) {
        NSApplication.shared.applicationIconImage = image
      } else {
        NSApplication.shared.applicationIconImage = nil
      }
      result(nil)
    }
  }
}

/// How long since the keyboard or mouse was last used, for away when idle
/// (lib/src/desktop/idle.dart).
enum Idle {
  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "anvil/idle", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      guard call.method == "idleSeconds" else {
        result(FlutterMethodNotImplemented)
        return
      }
      let anyInput = CGEventType(rawValue: ~0)!
      result(CGEventSource.secondsSinceLastEventType(.combinedSessionState, eventType: anyInput))
    }
  }
}

/// The channel launch_at_startup calls on macOS, answered by the system's
/// login items (SMAppService, macOS 13 and later; refused before).
enum StartAtLogin {
  static func register(with messenger: FlutterBinaryMessenger) {
    let channel = FlutterMethodChannel(name: "launch_at_startup", binaryMessenger: messenger)
    channel.setMethodCallHandler { call, result in
      guard #available(macOS 13.0, *) else {
        result(FlutterError(code: "unsupported", message: "start at login needs macOS 13", details: nil))
        return
      }
      switch call.method {
      case "launchAtStartupIsEnabled":
        result(SMAppService.mainApp.status == .enabled)
      case "launchAtStartupSetEnabled":
        let on = (call.arguments as? [String: Any])?["setEnabledValue"] as? Bool ?? false
        do {
          if on {
            try SMAppService.mainApp.register()
          } else {
            try SMAppService.mainApp.unregister()
          }
          result(nil)
        } catch {
          result(FlutterError(code: "failed", message: error.localizedDescription, details: nil))
        }
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }
}
