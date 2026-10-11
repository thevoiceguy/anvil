import Foundation

/// Native code's way to the phone in the Rust core, with or without Dart: a
/// VoIP push starts it in a process where Flutter may never run (FCP's
/// `docs/PUSH.md` §2.5). The phone's calls come here as events for CallKit
/// (`CallSystem.fromPhone`); what the user does at CallKit goes back
/// (`act`). The Rust side is `app/rust/src/ios.rs` and `system.rs`.
enum PhoneLink {
  /// The Rust calls block: one at a time, never on the main thread.
  private static let worker = DispatchQueue(label: "com.thevoiceguy.anvil.phone")

  private static var following = false

  /// Where the app keeps its session (as Dart's path_provider has it: the
  /// application support directory).
  static var sessionPath: String {
    let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    return support.appendingPathComponent("session.json").path
  }

  static var signedIn: Bool { FileManager.default.fileExists(atPath: sessionPath) }

  /// A string the core answered with, freed; null is none.
  private static func take(_ answer: UnsafeMutablePointer<CChar>?) -> String? {
    guard let answer = answer else { return nil }
    defer { anvil_string_free(answer) }
    return String(cString: answer)
  }

  /// Follow the phone's calls for `calls`, once per process.
  static func start(_ calls: CallSystem) {
    dispatchPrecondition(condition: .onQueue(.main))
    if following { return }
    following = true
    let reader = Thread {
      while true {
        guard let json = take(anvil_next_call_event(60_000)) else { continue }
        guard let data = json.data(using: .utf8),
          let event = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else {
          NSLog("AnvilPhone: an unreadable call event: %@", json)
          continue
        }
        DispatchQueue.main.async { calls.fromPhone(event) }
      }
    }
    reader.name = "anvil-calls"
    reader.start()
  }

  /// Woken by a push: start the phone from the saved session, or register
  /// again if it runs. `failed` hears why it could not, on the main thread.
  static func wake(failed: @escaping (String) -> Void) {
    let session = sessionPath
    worker.async {
      guard let error = take(anvil_wake(session)) else { return }
      NSLog("AnvilPhone: the phone did not wake: %@", error)
      DispatchQueue.main.async { failed(error) }
    }
  }

  /// Where this install is woken from now on; an empty token is nowhere.
  static func setPush(provider: String, param: String, token: String) {
    let session = sessionPath
    worker.async {
      if let error = take(anvil_set_push(session, provider, param, token)) {
        NSLog("AnvilPhone: the push address was not kept: %@", error)
      }
    }
  }

  /// What the user did at the system's call screen, for the phone.
  static func act(_ action: String, _ id: Int64, _ extra: [String: Any] = [:]) {
    var body = extra
    body["action"] = action
    body["id"] = NSNumber(value: id)
    guard let data = try? JSONSerialization.data(withJSONObject: body),
      let json = String(data: data, encoding: .utf8)
    else { return }
    worker.async {
      if let error = take(anvil_act(json)) {
        NSLog("AnvilPhone: %@ on call %lld: %@", action, id, error)
      }
    }
  }
}
