import Foundation

/// Wire constants. Normative source is ../../docs/PROTOCOL.md; this file and
/// the Kotlin `Proto` object must agree byte for byte.
enum Proto {
    static let version = 1
    static let headerSize = 8
    static let maxPayload = 65535

    static let audio: UInt8 = 0
    static let ping: UInt8 = 1
    static let pong: UInt8 = 2
    static let stats: UInt8 = 3
    static let bye: UInt8 = 4
    static let control: UInt8 = 5

    /// Capability: audio in both directions plus CONTROL frames (PROTOCOL.md §6).
    static let capDuplex = "duplex"
}

struct FrameHeader {
    let type: UInt8
    let flags: UInt8
    let length: Int
    let seq: UInt32
}

/// Header fields are big-endian. AUDIO *payload* is little-endian PCM — it is
/// raw sample data straight from Core Audio, not a protocol integer.
func encodeHeader(type: UInt8, flags: UInt8 = 0, length: Int, seq: UInt32) -> Data {
    precondition(length >= 0 && length <= Proto.maxPayload, "payload \(length) out of range")
    var data = Data(capacity: Proto.headerSize)
    data.append(type)
    data.append(flags)
    data.append(UInt8((length >> 8) & 0xFF))
    data.append(UInt8(length & 0xFF))
    data.append(UInt8((seq >> 24) & 0xFF))
    data.append(UInt8((seq >> 16) & 0xFF))
    data.append(UInt8((seq >> 8) & 0xFF))
    data.append(UInt8(seq & 0xFF))
    return data
}

func decodeHeader(_ data: Data) -> FrameHeader? {
    guard data.count >= Proto.headerSize else { return nil }
    let b = [UInt8](data.prefix(Proto.headerSize))
    let length = Int(b[2]) << 8 | Int(b[3])
    let seq = UInt32(b[4]) << 24 | UInt32(b[5]) << 16 | UInt32(b[6]) << 8 | UInt32(b[7])
    return FrameHeader(type: b[0], flags: b[1], length: length, seq: seq)
}

func frame(type: UInt8, payload: Data, seq: UInt32) -> Data {
    var out = encodeHeader(type: type, length: payload.count, seq: seq)
    out.append(payload)
    return out
}

func putU64BE(_ value: UInt64) -> Data {
    var data = Data(capacity: 8)
    for shift in stride(from: 56, through: 0, by: -8) {
        data.append(UInt8((value >> UInt64(shift)) & 0xFF))
    }
    return data
}

func monotonicMicros() -> UInt64 {
    var timebase = mach_timebase_info_data_t()
    mach_timebase_info(&timebase)
    let nanos = mach_absolute_time() &* UInt64(timebase.numer) / UInt64(timebase.denom)
    return nanos / 1000
}
