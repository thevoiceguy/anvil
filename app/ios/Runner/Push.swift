import Foundation
import PushKit

/// FCP wakes the app with a VoIP push through APNs (FCP's `docs/PUSH.md`):
/// the app registers its PushKit token as its push address, and FCP pushes
/// a call to it when the app is asleep. A call push rings at once on
/// CallKit — Apple's rule: every VoIP push reports a call, or iOS stops
/// delivering them — while the phone starts and registers, and the call's
/// INVITE then takes the ringing call's place (`CallSystem`).
///
/// The topic is the app's bundle ID with `.voip`, under the team the build
/// is signed by (`AnvilApnsTeam` in Info.plist, from `DEVELOPMENT_TEAM`);
/// an unsigned build has no team and registers without a push address.
final class Push: NSObject, PKPushRegistryDelegate {
  private let calls: CallSystem
  private let registry = PKPushRegistry(queue: .main)

  /// Made at launch, as PushKit requires, so a push that launched the app
  /// in the background is delivered.
  init(calls: CallSystem) {
    self.calls = calls
    super.init()
    registry.delegate = self
    registry.desiredPushTypes = [.voIP]
  }

  /// RFC 8599's `pn-param` for APNs: the team, a dot, the topic; none in a
  /// build with no team.
  static var param: String? {
    let team = Bundle.main.object(forInfoDictionaryKey: "AnvilApnsTeam") as? String ?? ""
    guard !team.isEmpty, !team.hasPrefix("$("), let bundle = Bundle.main.bundleIdentifier else {
      return nil
    }
    return "\(team).\(bundle).voip"
  }

  func pushRegistry(
    _ registry: PKPushRegistry, didUpdate credentials: PKPushCredentials, for type: PKPushType
  ) {
    guard type == .voIP, let param = Push.param else { return }
    let token = credentials.token.map { String(format: "%02x", $0) }.joined()
    PhoneLink.setPush(provider: "apns", param: param, token: token)
  }

  func pushRegistry(_ registry: PKPushRegistry, didInvalidatePushTokenFor type: PKPushType) {
    guard type == .voIP else { return }
    PhoneLink.setPush(provider: "apns", param: Push.param ?? "", token: "")
  }

  func pushRegistry(
    _ registry: PKPushRegistry, didReceiveIncomingPushWith payload: PKPushPayload,
    for type: PKPushType, completion: @escaping () -> Void
  ) {
    guard type == .voIP else {
      completion()
      return
    }
    handle(payload.dictionaryPayload, completion: completion)
  }

  /// A push's body (`type`, `call`, `from`, `name`, `tenant`, `sent`);
  /// `completion` once CallKit has been told.
  func handle(_ data: [AnyHashable: Any], completion: @escaping () -> Void) {
    let call = data["call"] as? String ?? ""
    let name = data["name"] as? String ?? ""
    let from = data["from"] as? String ?? ""
    switch data["type"] as? String {
    case "call":
      // Signed out since the token was registered: shown and gone.
      guard PhoneLink.signedIn else {
        NSLog("AnvilPush: a call push for an app signed out")
        calls.reportAndEnd(name: name, number: from, completion: completion)
        return
      }
      calls.reportPushed(serverCall: call, name: name, number: from, completion: completion)
      PhoneLink.wake { [calls] _ in calls.wakeFailed(serverCall: call) }
    case "cancel":
      calls.cancelPushed(serverCall: call, completion: completion)
    case "test":
      // An administrator's test: CallKit shows it, and it is gone.
      calls.reportAndEnd(
        name: NSLocalizedString(
          "Anvil can be woken for calls", comment: "An administrator's test push, shown briefly"),
        number: "", completion: completion)
    default:
      NSLog("AnvilPush: a push of a kind this app does not know: %@", "\(data["type"] ?? "")")
      calls.reportAndEnd(name: name, number: from, completion: completion)
    }
  }
}
