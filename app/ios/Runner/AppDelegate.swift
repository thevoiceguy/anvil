import AVFoundation
import CallKit
import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  private let calls = CallSystem()

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
      calls.channel = channel
      let calls = self.calls
      channel.setMethodCallHandler { call, result in
        let args = call.arguments as? [String: Any] ?? [:]
        let id = (args["id"] as? NSNumber)?.int64Value ?? -1
        let name = args["name"] as? String ?? ""
        let number = args["number"] as? String ?? ""
        switch call.method {
        case "requestMicrophone": AppDelegate.requestMicrophone(result)
        case "startAudio": AppDelegate.startAudio(result)
        case "reportIncoming":
          calls.reportIncoming(id: id, name: name, number: number)
          result(nil)
        case "reportOutgoing":
          calls.reportOutgoing(id: id, name: name, number: number)
          result(nil)
        case "reportConnected":
          calls.reportConnected(id: id)
          result(nil)
        case "reportHeld":
          calls.reportHeld(id: id, on: args["on"] as? Bool ?? false)
          result(nil)
        case "reportEnded":
          calls.reportEnded(id: id)
          result(nil)
        default: result(FlutterMethodNotImplemented)
        }
      }
    }
  }

  /// A phone's audio: the microphone and the earpiece together, with the
  /// system's echo cancellation (cpal leaves the session as it finds it,
  /// and the default plays only). Per-call routing comes with U4c.
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

/// The app's calls as CallKit knows them: the system's call screen rings
/// for a call coming in (on the lock screen too), shows a call placed, and
/// hands back what the user does there — answer, end, hold, mute, a key —
/// as `callEvent`s on `anvil/mobile` (lib/src/mobile/platform.dart).
final class CallSystem: NSObject, CXProviderDelegate {
  var channel: FlutterMethodChannel?
  private let provider: CXProvider
  private let controller = CXCallController()
  private var uuids: [Int64: UUID] = [:]
  private var ids: [UUID: Int64] = [:]
  private var incoming: Set<Int64> = []
  /// Calls answered at the system's screen (not to be answered again).
  private var answered: Set<Int64> = []
  /// Calls the system took as placed.
  private var started: [Int64: (name: String, number: String)] = [:]

  override init() {
    let config = CXProviderConfiguration()
    config.supportsVideo = false
    config.maximumCallGroups = 2
    config.maximumCallsPerCallGroup = 1
    config.supportedHandleTypes = [.generic, .phoneNumber]
    // The app keeps its own history (FCP's), so the system's Recents would
    // show each call twice.
    config.includesCallsInRecents = false
    provider = CXProvider(configuration: config)
    super.init()
    provider.setDelegate(self, queue: nil)
  }

  private func emit(_ action: String, _ id: Int64, _ extra: [String: Any] = [:]) {
    var event = extra
    event["action"] = action
    event["id"] = NSNumber(value: id)
    DispatchQueue.main.async { self.channel?.invokeMethod("callEvent", arguments: event) }
  }

  private func remember(_ id: Int64) -> UUID {
    let uuid = UUID()
    uuids[id] = uuid
    ids[uuid] = id
    return uuid
  }

  private func forget(_ id: Int64) {
    if let uuid = uuids.removeValue(forKey: id) { ids.removeValue(forKey: uuid) }
    incoming.remove(id)
    answered.remove(id)
    started.removeValue(forKey: id)
  }

  private func update(name: String, number: String) -> CXCallUpdate {
    let update = CXCallUpdate()
    update.remoteHandle = CXHandle(type: .generic, value: number)
    update.localizedCallerName = name.isEmpty ? number : name
    update.hasVideo = false
    update.supportsHolding = true
    update.supportsDTMF = true
    update.supportsGrouping = false
    update.supportsUngrouping = false
    return update
  }

  func reportIncoming(id: Int64, name: String, number: String) {
    let uuid = remember(id)
    incoming.insert(id)
    provider.reportNewIncomingCall(with: uuid, update: update(name: name, number: number)) {
      error in
      DispatchQueue.main.async {
        guard let error = error else {
          self.emit("shown", id)
          return
        }
        self.forget(id)
        var reason = "unavailable"
        if let error = error as? CXErrorCodeIncomingCallError,
          error.code == .filteredByDoNotDisturb || error.code == .filteredByBlockList
        {
          reason = "filtered"
        }
        self.emit("failed", id, ["reason": reason])
      }
    }
  }

  func reportOutgoing(id: Int64, name: String, number: String) {
    let uuid = remember(id)
    started[id] = (name, number)
    let action = CXStartCallAction(call: uuid, handle: CXHandle(type: .generic, value: number))
    controller.request(CXTransaction(action: action)) { error in
      guard error != nil else { return }
      DispatchQueue.main.async {
        self.forget(id)
        self.emit("failed", id, ["reason": "unavailable"])
      }
    }
  }

  func reportConnected(id: Int64) {
    guard let uuid = uuids[id] else { return }
    if incoming.contains(id) {
      // Answered in the app: the system's screen is told too.
      if !answered.contains(id) {
        answered.insert(id)
        controller.request(CXTransaction(action: CXAnswerCallAction(call: uuid))) { _ in }
      }
    } else {
      provider.reportOutgoingCall(with: uuid, connectedAt: nil)
    }
  }

  func reportHeld(id: Int64, on: Bool) {
    guard let uuid = uuids[id] else { return }
    controller.request(CXTransaction(action: CXSetHeldCallAction(call: uuid, onHold: on))) { _ in }
  }

  func reportEnded(id: Int64) {
    guard let uuid = uuids[id] else { return }
    forget(id)
    provider.reportCall(with: uuid, endedAt: nil, reason: .remoteEnded)
  }

  // MARK: CXProviderDelegate

  func providerDidReset(_ provider: CXProvider) {
    for id in Array(uuids.keys) {
      forget(id)
      emit("end", id)
    }
  }

  func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
    guard let id = ids[action.callUUID], let call = started[id] else {
      action.fail()
      return
    }
    provider.reportCall(with: action.callUUID, updated: update(name: call.name, number: call.number))
    provider.reportOutgoingCall(with: action.callUUID, startedConnectingAt: nil)
    action.fulfill()
    emit("shown", id)
  }

  func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
    guard let id = ids[action.callUUID] else {
      action.fail()
      return
    }
    let fromSystem = !answered.contains(id)
    answered.insert(id)
    action.fulfill()
    if fromSystem { emit("answer", id) }
  }

  func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
    guard let id = ids[action.callUUID] else {
      action.fail()
      return
    }
    forget(id)
    action.fulfill()
    emit("end", id)
  }

  func provider(_ provider: CXProvider, perform action: CXSetHeldCallAction) {
    guard let id = ids[action.callUUID] else {
      action.fail()
      return
    }
    action.fulfill()
    emit("hold", id, ["on": action.isOnHold])
  }

  func provider(_ provider: CXProvider, perform action: CXSetMutedCallAction) {
    guard let id = ids[action.callUUID] else {
      action.fail()
      return
    }
    action.fulfill()
    emit("mute", id, ["on": action.isMuted])
  }

  func provider(_ provider: CXProvider, perform action: CXPlayDTMFCallAction) {
    guard let id = ids[action.callUUID] else {
      action.fail()
      return
    }
    action.fulfill()
    emit("dtmf", id, ["digits": action.digits])
  }

  func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) {}

  func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) {}
}
