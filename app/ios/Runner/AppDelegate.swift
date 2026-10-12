import AVFoundation
import CallKit
import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  private let calls = CallSystem()
  private let routes = AudioRoutes()
  private var push: Push?

  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    // Before anything else: a VoIP push may have launched the app in the
    // background, with no Flutter to come (FCP's docs/PUSH.md §2.5).
    PhoneLink.start(calls)
    push = Push(calls: calls)
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    // `anvil/mobile`: what the app asks of the phone (lib/src/mobile).
    if let registrar = engineBridge.pluginRegistry.registrar(forPlugin: "AnvilMobile") {
      let channel = FlutterMethodChannel(
        name: "anvil/mobile", binaryMessenger: registrar.messenger())
      calls.channel = channel
      routes.channel = channel
      let calls = self.calls
      let routes = self.routes
      let push = self.push
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
        case "audioRoutes": result(routes.current())
        case "setAudioRoute":
          do {
            try routes.set(args["route"] as? String ?? "")
            result(nil)
          } catch {
            result(FlutterError(code: "route", message: error.localizedDescription, details: nil))
          }
        case "setProximity":
          UIDevice.current.isProximityMonitoringEnabled = args["on"] as? Bool ?? false
          result(nil)
        case "reportEnded":
          calls.reportEnded(id: id)
          result(nil)
        case "deliverPush":
          // A push as PushKit would hand it, for the app's tests on a
          // simulator, which PushKit does not reach (debug builds only).
          #if DEBUG
            push?.handle(args) {}
            result(nil)
          #else
            result(FlutterMethodNotImplemented)
          #endif
        default: result(FlutterMethodNotImplemented)
        }
      }
    }
  }

  /// A phone's audio: the microphone and the earpiece together, with the
  /// system's echo cancellation (cpal leaves the session as it finds it,
  /// and the default plays only). Where it goes: `AudioRoutes`.
  static func prepareAudio() throws {
    try AVAudioSession.sharedInstance().setCategory(
      .playAndRecord, mode: .voiceChat, options: [.allowBluetooth])
  }

  private static func startAudio(_ result: @escaping FlutterResult) {
    do {
      try prepareAudio()
      try AVAudioSession.sharedInstance().setActive(true)
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

/// The app's calls as CallKit knows them: the phone in the Rust core tells
/// of each call (`fromPhone`, through `PhoneLink`), the system's call screen
/// rings for a call coming in (on the lock screen too), shows a call placed,
/// and hands back what the user does there — answer, end, hold, mute, a key
/// — to the phone, and to Dart when it runs, as `callEvent`s on
/// `anvil/mobile` (lib/src/mobile/platform.dart).
///
/// A call a VoIP push told of (FCP's `docs/PUSH.md` §2.5) rings at once,
/// before the phone has woken and registered; the phone's call, when its
/// INVITE comes, takes its place — the one FCP names as the push did.
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
  /// The server's id for a call, as a push or its INVITE named it.
  private var serverCalls: [Int64: String] = [:]
  /// Pushed calls answered at the screen before the phone's call came.
  private var answerWhenReady: Set<Int64> = []

  /// A pushed call rings this long without the phone's call, then stops.
  private static let pushedRing: TimeInterval = 60
  /// Ids for pushed calls: below zero, apart from the phone's.
  private var nextPushed: Int64 = -1
  /// Calls declined from a push before their INVITE came: when.
  private var declined: [String: Date] = [:]

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

  /// Dart hears what happened at the system, when it runs (its tests do).
  private func tell(_ action: String, _ id: Int64, _ extra: [String: Any] = [:]) {
    var event = extra
    event["action"] = action
    event["id"] = NSNumber(value: id)
    DispatchQueue.main.async { self.channel?.invokeMethod("callEvent", arguments: event) }
  }

  /// What the user or the system did with a call: for the phone, and Dart.
  /// A pushed call's phone call is not here yet: nothing to act on.
  private func emit(_ action: String, _ id: Int64, _ extra: [String: Any] = [:]) {
    tell(action, id, extra)
    if action != "shown" && id >= 0 { PhoneLink.act(action, id, extra) }
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
    serverCalls.removeValue(forKey: id)
    answerWhenReady.remove(id)
  }

  /// The pushed call `from` becomes the phone's call `id`; whether it was
  /// answered while it waited.
  private func adopt(_ from: Int64, as id: Int64) -> Bool {
    guard let uuid = uuids.removeValue(forKey: from) else { return false }
    uuids[id] = uuid
    ids[uuid] = id
    if incoming.remove(from) != nil { incoming.insert(id) }
    if answered.remove(from) != nil { answered.insert(id) }
    if let serverCall = serverCalls.removeValue(forKey: from) { serverCalls[id] = serverCall }
    return answerWhenReady.remove(from) != nil
  }

  private func decline(_ serverCall: String) {
    let now = Date()
    declined = declined.filter { now.timeIntervalSince($0.value) < CallSystem.pushedRing }
    declined[serverCall] = now
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

  /// What the phone says of its calls (`app/rust/src/system.rs`).
  func fromPhone(_ event: [String: Any]) {
    let id = (event["id"] as? NSNumber)?.int64Value ?? -1
    let name = event["name"] as? String ?? ""
    let number = event["number"] as? String ?? ""
    switch event["kind"] as? String {
    case "incoming":
      phoneRinging(
        id: id, name: name, number: number, serverCall: event["server_call"] as? String)
    case "outgoing":
      if uuids[id] == nil { reportOutgoing(id: id, name: name, number: number) }
    case "connected": reportConnected(id: id)
    case "held": reportHeld(id: id, on: (event["on"] as? NSNumber)?.boolValue ?? false)
    case "ended": reportEnded(id: id)
    // The phone stopped: its calls are over; a pushed call waits on.
    case "reset":
      for id in uuids.keys where id >= 0 { reportEnded(id: id) }
    default: break
    }
  }

  /// The phone's call ringing in: the pushed call it is, or a new one.
  private func phoneRinging(id: Int64, name: String, number: String, serverCall: String?) {
    if uuids[id] != nil { return }
    if let serverCall = serverCall, declined.removeValue(forKey: serverCall) != nil {
      PhoneLink.act("end", id)
      return
    }
    // FCP names its calls; a server that does not, the call a push showed
    // first (pushed ids count down from -1).
    let pushed = uuids.keys.filter { $0 < 0 }.sorted(by: >)
    let match: Int64?
    if let serverCall = serverCall {
      match = pushed.first { serverCalls[$0] == serverCall }
    } else {
      match = pushed.first
    }
    guard let from = match, let uuid = uuids[from] else {
      reportIncoming(id: id, name: name, number: number)
      if let serverCall = serverCall { serverCalls[id] = serverCall }
      return
    }
    let answer = adopt(from, as: id)
    // The INVITE says who is calling, where a minimal push did not.
    provider.reportCall(with: uuid, updated: update(name: name, number: number))
    if answer { PhoneLink.act("answer", id) }
  }

  func reportIncoming(
    id: Int64, name: String, number: String, completion: (() -> Void)? = nil
  ) {
    let uuid = remember(id)
    incoming.insert(id)
    provider.reportNewIncomingCall(with: uuid, update: update(name: name, number: number)) {
      error in
      DispatchQueue.main.async {
        defer { completion?() }
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

  /// A push says a call is coming (`serverCall`, FCP's id for it): it rings
  /// now, while the phone wakes and registers; `completion` once CallKit has
  /// it.
  func reportPushed(
    serverCall: String, name: String, number: String, completion: @escaping () -> Void
  ) {
    // Pushed again, or ringing already: told to CallKit as the same call.
    if let known = serverCalls.first(where: { $0.value == serverCall })?.key,
      let uuid = uuids[known]
    {
      provider.reportNewIncomingCall(with: uuid, update: update(name: name, number: number)) {
        _ in DispatchQueue.main.async { completion() }
      }
      return
    }
    if declined[serverCall] != nil {
      reportAndEnd(name: name, number: number, completion: completion)
      return
    }
    let id = nextPushed
    nextPushed -= 1
    serverCalls[id] = serverCall
    reportIncoming(id: id, name: name, number: number, completion: completion)
    DispatchQueue.main.asyncAfter(deadline: .now() + CallSystem.pushedRing) {
      if self.uuids[id] != nil {
        NSLog("AnvilCalls: the call %@ never came", serverCall)
        self.reportEnded(id: id, reason: .unanswered)
      }
    }
  }

  /// The call a push told of is over (answered elsewhere, or given up).
  /// The push still reports a call, as every VoIP push must.
  func cancelPushed(serverCall: String, completion: @escaping () -> Void) {
    guard let id = serverCalls.first(where: { $0.value == serverCall })?.key,
      let uuid = uuids[id]
    else {
      reportAndEnd(name: "", number: "", completion: completion)
      return
    }
    provider.reportNewIncomingCall(with: uuid, update: CXCallUpdate()) { _ in
      DispatchQueue.main.async {
        if id < 0 { self.reportEnded(id: id, reason: .answeredElsewhere) }
        completion()
      }
    }
  }

  /// The phone could not wake: the pushed call cannot reach it.
  func wakeFailed(serverCall: String) {
    guard let id = serverCalls.first(where: { $0.key < 0 && $0.value == serverCall })?.key
    else { return }
    reportEnded(id: id, reason: .failed)
  }

  /// A push that rings nothing still reports a call to CallKit (Apple's
  /// rule), ended at once.
  func reportAndEnd(name: String, number: String, completion: @escaping () -> Void) {
    let uuid = UUID()
    provider.reportNewIncomingCall(with: uuid, update: update(name: name, number: number)) {
      error in
      DispatchQueue.main.async {
        if error == nil { self.provider.reportCall(with: uuid, endedAt: nil, reason: .remoteEnded) }
        completion()
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

  func reportEnded(id: Int64, reason: CXCallEndedReason = .remoteEnded) {
    guard let uuid = uuids[id] else { return }
    forget(id)
    provider.reportCall(with: uuid, endedAt: nil, reason: reason)
    tell("ended", id)
  }

  // MARK: CXProviderDelegate

  /// The system dropped every call it had (its call service restarted; a
  /// simulator has none and resets at the first call ringing in). The user
  /// ended nothing: the calls go on in the app, without the system's screen.
  func providerDidReset(_ provider: CXProvider) {
    for id in Array(uuids.keys) {
      forget(id)
      emit("failed", id, ["reason": "reset"])
    }
  }

  func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
    guard let id = ids[action.callUUID], let call = started[id] else {
      action.fail()
      return
    }
    try? AppDelegate.prepareAudio()
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
    // CallKit activates the session for the call once this is fulfilled:
    // it must be a call's session by then (the app may not be in front).
    try? AppDelegate.prepareAudio()
    let fromSystem = !answered.contains(id)
    answered.insert(id)
    if id < 0 { answerWhenReady.insert(id) }
    action.fulfill()
    if fromSystem { emit("answer", id) }
  }

  func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
    guard let id = ids[action.callUUID] else {
      action.fail()
      return
    }
    // A pushed call declined before its INVITE came: refused when it does.
    if id < 0, let serverCall = serverCalls[id] { decline(serverCall) }
    forget(id)
    action.fulfill()
    emit("end", id)
    if id < 0 { tell("ended", id) }
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

/// Where a call's audio goes on iOS: the session's route (the earpiece,
/// the speaker, a Bluetooth headset or car, a wired headset), told to Dart
/// at every change as `{current, available, bluetoothName}`
/// (lib/src/mobile/platform.dart), and changed by overriding the output or
/// choosing the input whose port carries the output with it.
final class AudioRoutes: NSObject {
  var channel: FlutterMethodChannel?

  override init() {
    super.init()
    NotificationCenter.default.addObserver(
      self, selector: #selector(changed),
      name: AVAudioSession.routeChangeNotification, object: nil)
  }

  @objc private func changed(_ note: Notification) {
    DispatchQueue.main.async {
      self.channel?.invokeMethod("audioRoutes", arguments: self.current())
    }
  }

  private static let bluetooth: [AVAudioSession.Port] = [.bluetoothHFP, .bluetoothLE]

  func current() -> [String: Any] {
    let session = AVAudioSession.sharedInstance()
    let output = session.currentRoute.outputs.first
    let inputs = session.availableInputs ?? []
    let current: String
    switch output?.portType {
    case .builtInSpeaker?: current = "speaker"
    case .bluetoothHFP?, .bluetoothA2DP?, .bluetoothLE?, .carAudio?: current = "bluetooth"
    case .headphones?, .usbAudio?: current = "wired"
    default: current = "earpiece"
    }
    let headset = inputs.first { AudioRoutes.bluetooth.contains($0.portType) }
    let wired = inputs.contains { $0.portType == .headsetMic } || current == "wired"
    var available: [String] = []
    // A wired headset takes the earpiece's place; an iPad has none.
    if UIDevice.current.userInterfaceIdiom == .phone && !wired { available.append("earpiece") }
    available.append("speaker")
    if headset != nil || current == "bluetooth" { available.append("bluetooth") }
    if wired { available.append("wired") }
    var routes: [String: Any] = ["current": current, "available": available]
    if let name = headset?.portName ?? (current == "bluetooth" ? output?.portName : nil) {
      routes["bluetoothName"] = name
    }
    return routes
  }

  func set(_ route: String) throws {
    let session = AVAudioSession.sharedInstance()
    let inputs = session.availableInputs ?? []
    switch route {
    case "speaker":
      try session.overrideOutputAudioPort(.speaker)
    case "earpiece":
      try session.overrideOutputAudioPort(.none)
      try session.setPreferredInput(inputs.first { $0.portType == .builtInMic })
    case "bluetooth":
      try session.overrideOutputAudioPort(.none)
      try session.setPreferredInput(inputs.first { AudioRoutes.bluetooth.contains($0.portType) })
    case "wired":
      try session.overrideOutputAudioPort(.none)
      try session.setPreferredInput(inputs.first { $0.portType == .headsetMic })
    default:
      break
    }
  }
}
