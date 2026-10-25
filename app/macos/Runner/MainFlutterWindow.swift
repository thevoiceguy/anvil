import Cocoa
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

    super.awakeFromNib()
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
