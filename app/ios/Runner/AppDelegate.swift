import AVFoundation
import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    // `anvil/mobile`: what the app asks of the phone (lib/src/mobile).
    if let registrar = engineBridge.pluginRegistry.registrar(forPlugin: "AnvilMobile") {
      let channel = FlutterMethodChannel(
        name: "anvil/mobile", binaryMessenger: registrar.messenger())
      channel.setMethodCallHandler { call, result in
        switch call.method {
        case "requestMicrophone": AppDelegate.requestMicrophone(result)
        case "startAudio": AppDelegate.startAudio(result)
        default: result(FlutterMethodNotImplemented)
        }
      }
    }
  }

  /// A phone's audio: the microphone and the earpiece together, with the
  /// system's echo cancellation (cpal leaves the session as it finds it,
  /// and the default plays only). Per-call routing comes with U4b.
  private static func startAudio(_ result: @escaping FlutterResult) {
    let session = AVAudioSession.sharedInstance()
    do {
      try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.allowBluetooth])
      try session.setActive(true)
      result(nil)
    } catch {
      result(FlutterError(code: "audio", message: error.localizedDescription, details: nil))
    }
  }

  /// Asks once (the system remembers the answer); says whether the app
  /// may use the microphone.
  private static func requestMicrophone(_ result: @escaping FlutterResult) {
    let answer: (Bool) -> Void = { granted in
      DispatchQueue.main.async { result(granted) }
    }
    if #available(iOS 17.0, *) {
      AVAudioApplication.requestRecordPermission(completionHandler: answer)
    } else {
      AVAudioSession.sharedInstance().requestRecordPermission(answer)
    }
  }
}
