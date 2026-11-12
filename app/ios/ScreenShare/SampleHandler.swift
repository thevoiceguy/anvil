import CoreMedia
import CoreVideo
import Darwin
import Foundation
import ReplayKit

/// The phone's screen for a share in Anvil (FCP's `docs/SOFTPHONE_VIDEO.md`,
/// A5). iOS captures the whole screen only here, in a broadcast upload
/// extension the user starts from the system's picker; each picture goes
/// to the app over a Unix socket in the app group's shared container,
/// where a share listens (anvil-video's `broadcast.rs`, which reads it).
///
/// Each picture is a 32-byte header (little-endian) — `ANVS`, version 1,
/// the turn that makes it upright (degrees clockwise), width, height, the
/// Y and CbCr planes' row strides, the format (1: NV12 video range, 2:
/// full range), a reserved word — then the two planes as they are in
/// memory. At most 15 a second: the app repeats the last while the
/// screen is still. The broadcast ends when the app closes the socket
/// (the share stopped there).
final class SampleHandler: RPBroadcastSampleHandler {
  private var socket: Int32 = -1
  private var lastSent: CFAbsoluteTime = 0
  private let interval: CFAbsoluteTime = 1.0 / 15.0

  override func broadcastStarted(withSetupInfo setupInfo: [String: NSObject]?) {
    guard let path = SampleHandler.socketPath() else {
      fail("Anvil's shared folder is missing.")
      return
    }
    // The app listens from the moment Share is pressed; allow a few
    // seconds for it all the same.
    for _ in 0..<50 {
      if connect(to: path) { return }
      usleep(100_000)
    }
    fail("Start sharing from a room in Anvil.")
  }

  override func broadcastFinished() {
    closeSocket()
  }

  override func processSampleBuffer(
    _ sampleBuffer: CMSampleBuffer, with sampleBufferType: RPSampleBufferType
  ) {
    guard sampleBufferType == .video, socket >= 0,
      let pixels = CMSampleBufferGetImageBuffer(sampleBuffer)
    else { return }
    let now = CFAbsoluteTimeGetCurrent()
    if now - lastSent < interval { return }
    lastSent = now

    let format: UInt32
    switch CVPixelBufferGetPixelFormatType(pixels) {
    case kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange: format = 1
    case kCVPixelFormatType_420YpCbCr8BiPlanarFullRange: format = 2
    default: return
    }
    let orientation =
      (CMGetAttachment(
        sampleBuffer, key: RPVideoSampleOrientationKey as CFString, attachmentModeOut: nil)
        as? NSNumber)?.uint32Value ?? CGImagePropertyOrientation.up.rawValue
    let turn = SampleHandler.turn(orientation)

    CVPixelBufferLockBaseAddress(pixels, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(pixels, .readOnly) }
    guard CVPixelBufferGetPlaneCount(pixels) == 2,
      let y = CVPixelBufferGetBaseAddressOfPlane(pixels, 0),
      let uv = CVPixelBufferGetBaseAddressOfPlane(pixels, 1)
    else { return }
    let width = CVPixelBufferGetWidthOfPlane(pixels, 0)
    let height = CVPixelBufferGetHeightOfPlane(pixels, 0)
    let ys = CVPixelBufferGetBytesPerRowOfPlane(pixels, 0)
    let uvs = CVPixelBufferGetBytesPerRowOfPlane(pixels, 1)
    let uvRows = CVPixelBufferGetHeightOfPlane(pixels, 1)
    // The app reads (height + 1) / 2 chroma rows.
    guard uvRows >= (height + 1) / 2 else { return }

    var header = Data(capacity: 32)
    header.append(contentsOf: Array("ANVS".utf8))
    SampleHandler.put(&header, UInt16(1))
    SampleHandler.put(&header, turn)
    for v in [UInt32(width), UInt32(height), UInt32(ys), UInt32(uvs), format, 0] {
      SampleHandler.put(&header, v)
    }
    let sent = header.withUnsafeBytes { writeAll($0.baseAddress!, $0.count) }
      && writeAll(y, ys * height)
      && writeAll(uv, uvs * ((height + 1) / 2))
    if !sent {
      closeSocket()
      fail("You stopped sharing in Anvil.")
    }
  }

  /// The turn (degrees clockwise) that shows a picture of this
  /// orientation upright (`CGImagePropertyOrientation`).
  private static func turn(_ orientation: UInt32) -> UInt16 {
    switch CGImagePropertyOrientation(rawValue: orientation) ?? .up {
    case .down, .downMirrored: return 180
    case .right, .rightMirrored: return 90
    case .left, .leftMirrored: return 270
    default: return 0
    }
  }

  private static func put<T: FixedWidthInteger>(_ data: inout Data, _ value: T) {
    var le = value.littleEndian
    withUnsafeBytes(of: &le) { data.append(contentsOf: $0) }
  }

  /// The socket the app listens on, in the group's container.
  private static func socketPath() -> String? {
    guard let group = Bundle.main.object(forInfoDictionaryKey: "AnvilAppGroup") as? String,
      let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
    else { return nil }
    return dir.appendingPathComponent("anvil-share.sock").path
  }

  private func connect(to path: String) -> Bool {
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8CString)
    let room = MemoryLayout.size(ofValue: address.sun_path)
    guard bytes.count <= room else { return false }
    withUnsafeMutableBytes(of: &address.sun_path) { raw in
      bytes.withUnsafeBytes { raw.copyMemory(from: $0) }
    }
    let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    // A closed socket is an error to handle, not a signal that kills us.
    var on: Int32 = 1
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size))
    let connected = withUnsafePointer(to: &address) {
      $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
      }
    }
    if connected != 0 {
      Darwin.close(fd)
      return false
    }
    socket = fd
    return true
  }

  private func writeAll(_ from: UnsafeRawPointer, _ count: Int) -> Bool {
    var done = 0
    while done < count {
      let n = Darwin.write(socket, from + done, count - done)
      if n < 0 {
        if errno == EINTR { continue }
        return false
      }
      done += n
    }
    return true
  }

  private func closeSocket() {
    if socket >= 0 {
      Darwin.close(socket)
      socket = -1
    }
  }

  private func fail(_ reason: String) {
    finishBroadcastWithError(
      NSError(
        domain: "com.thevoiceguy.anvil.ScreenShare", code: 1,
        userInfo: [NSLocalizedFailureReasonErrorKey: reason]))
  }
}
