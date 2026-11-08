// Small native bridge derived from generic offline-compile/raw-readback patterns.
import Foundation
import Metal
import CryptoKit
import Dispatch

let width = 17, height = 11, innerWidth = 15, rowCount = 135
let pitch = 256, guardBytes = 256, traceGuard = 64
let targetBytes = guardBytes * 2 + pitch * height
struct Failure: Error { let reason: String }
func need(_ ok: Bool, _ reason: String) throws { if !ok { throw Failure(reason: reason) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func errorInfo(_ error: Error) -> [String: Any] {
    if let e = error as? Failure { return ["reason": e.reason] }
    let e = error as NSError
    return ["reason": "native_api_error", "domain": e.domain, "code": e.code]
}
func json(_ x: Any) throws -> Data { try JSONSerialization.data(withJSONObject: x, options: [.sortedKeys, .prettyPrinted]) }
func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
func swizzleInfo(_ s: MTLTextureSwizzleChannels) -> [Int] {
    [Int(s.red.rawValue), Int(s.green.rawValue), Int(s.blue.rawValue), Int(s.alpha.rawValue)]
}
let identity = MTLTextureSwizzleChannels(red: .red, green: .green, blue: .blue, alpha: .alpha)
func textureInfo(_ t: MTLTexture) -> [String: Any] {
    ["format": t.pixelFormat.rawValue, "storage": t.storageMode.rawValue, "usage": t.usage.rawValue,
     "type": t.textureType.rawValue, "width": t.width, "height": t.height, "depth": t.depth,
     "mips": t.mipmapLevelCount, "slices": t.arrayLength, "samples": t.sampleCount,
     "buffer_backed_property": t.buffer != nil, "buffer_offset": t.bufferOffset,
     "buffer_pitch": t.bufferBytesPerRow, "parent_present": t.parentTexture != nil,
     "parent_level": t.parentRelativeLevel, "parent_slice": t.parentRelativeSlice,
     "swizzle": swizzleInfo(t.swizzle), "framebuffer_only": t.isFramebufferOnly,
     "hazard_tracking": t.hazardTrackingMode.rawValue]
}
struct Spec: Decodable {
    let id: Int; let name: String; let precision: Int; let mode: String
    let mask: UInt; let fragment: String; let negative: String
}
struct Manifest: Decodable {
    let schema: Int; let width: Int; let height: Int; let pitch: Int
    let guardBytes: Int; let traceGuard: Int; let rowCount: Int; let q2: [Spec]
}
let entryPoints: Set<String> = ["read_float", "read_half", "sample_float", "sample_half", "wrong_read_float",
    "full_triangle", "output_float", "output_half", "wrong_kernel", "wrong_output"]

final class Session {
    let source: URL, out: URL, label: String
    var dev: MTLDevice!, queue: MTLCommandQueue!, library: MTLLibrary!
    var report: [String: Any] = ["schema": 1, "status": "FAILED_NOT_PASS", "cpu_result_fallback": false,
        "resource_substitution": false, "source_compile_fallback": false,
        "gpu_timing_note": "zero timestamps mean unavailable; no timing is invented"]
    var commands: [[String: Any]] = [], records: [[String: Any]] = []
    var raw = Data()
    init(_ source: URL, _ out: URL, _ label: String) {
        self.source = source; self.out = out; self.label = label
        report["probe"] = label; report["raw_file"] = label + ".gpu.bin"
    }
    func input(_ name: String, _ limit: Int = 1_048_576) throws -> Data {
        let d = try Data(contentsOf: out.appendingPathComponent(name))
        try need(d.count <= limit, "input_budget_" + name); return d
    }
    func checkpoint(_ stage: String) throws {
        report["last_stage"] = stage; report["commands"] = commands; report["records"] = records
        report["raw_bytes"] = raw.count; report["raw_sha256"] = sha(raw)
        let d = try json(report); try need(d.count <= 1_048_576, "report_budget")
        try d.write(to: out.appendingPathComponent(label + ".json"))
    }
    func save(_ d: Data, _ fields: [String: Any]) throws {
        try need(records.count < 96 && raw.count + d.count <= 1_048_576, "raw_budget")
        var r = fields; r["offset"] = raw.count; r["length"] = d.count; r["sha256"] = sha(d)
        raw.append(d); records.append(r)
        // Only real, completed GPU readbacks reach this file; no repairs/predictions.
        try raw.write(to: out.appendingPathComponent(label + ".gpu.bin"))
        try checkpoint("saved_" + (fields["name"] as? String ?? "record"))
    }
    func prepare() throws {
        try checkpoint("before_device")
        var endian: UInt16 = 1
        try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little_endian_required")
        guard let d = MTLCreateSystemDefaultDevice() else { throw Failure(reason: "device_nil_unsupported_no_substitute") }
        dev = d
        var info: [String: Any] = ["name": d.name, "registry_id": String(d.registryID),
            "unified_memory": d.hasUnifiedMemory, "low_power": d.isLowPower, "removable": d.isRemovable,
            "max_buffer_length": d.maxBufferLength, "selection": "system_default_only",
            "mac2": d.supportsFamily(.mac2), "apple7": d.supportsFamily(.apple7), "metal3": d.supportsFamily(.metal3)]
        if #available(macOS 14.0, *) { info["architecture"] = d.architecture.name }
        report["device"] = info
        report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
        report["os_build_from_runner_sw_vers"] = ProcessInfo.processInfo.environment["NATIVE_OS_BUILD"] ?? "missing"
        report["metadata_file"] = "metadata.json"
        try checkpoint("device_recorded")
        guard let q = d.makeCommandQueue() else { throw Failure(reason: "queue_nil_unsupported") }; queue = q
        let air = try input("probe.air"), lib = try input("probe.metallib")
        let pair = try JSONSerialization.jsonObject(with: input("compile-pair.json")) as? [String: Any]
        let msl = try Data(contentsOf: source.appendingPathComponent("probe.metal"))
        try need(!air.isEmpty && lib.prefix(4) == Data([0x4d, 0x54, 0x4c, 0x42]) &&
            pair?["source_sha256"] as? String == sha(msl) && pair?["air_sha256"] as? String == sha(air) &&
            pair?["metallib_sha256"] as? String == sha(lib) && pair?["compile_exit"] as? Int == 0 &&
            pair?["link_exit"] as? Int == 0 && pair?["link_input"] as? String == "probe.air", "offline_compile_chain")
        library = try d.makeLibrary(data: lib.withUnsafeBytes { DispatchData(bytes: $0) })
        try need(Set(library.functionNames) == entryPoints, "offline_entry_points")
        report["loaded_metallib_sha256"] = sha(lib); report["compile_pair"] = pair
        let md = try input("cases.json", 131072)
        let m = try JSONDecoder().decode(Manifest.self, from: md)
        try need(m.schema == 1 && m.width == width && m.height == height && m.pitch == pitch &&
            m.guardBytes == guardBytes && m.traceGuard == traceGuard && m.rowCount == rowCount, "input_layout")
        report["input_manifest_sha256"] = sha(md)
        try need(MTLPixelFormat.a8Unorm.rawValue == 1 && MTLPixelFormat.r8Unorm.rawValue == 10 &&
            MTLPixelFormat.rg16Float.rawValue == 65 && MTLColorWriteMask.red.rawValue == 8 &&
            MTLColorWriteMask.green.rawValue == 4 && MTLBlendFactor.destinationColor.rawValue == 6 &&
            MTLBlendFactor.destinationAlpha.rawValue == 8 && MTLCommandBufferStatus.completed.rawValue == 4,
            "api_enum_contract")
        try checkpoint("prepared")
    }
    func buffer(_ length: Int, _ mode: MTLStorageMode = .shared) throws -> MTLBuffer {
        try need((1...1_048_576).contains(length), "buffer_length")
        let option: MTLResourceOptions
        switch mode { case .shared: option = .storageModeShared
        case .managed: option = .storageModeManaged
        case .private: option = .storageModePrivate
        default: throw Failure(reason: "storage_not_a_buffer_class") }
        report["pending_buffer"] = ["length": length, "requested_storage": mode.rawValue]
        try checkpoint("before_buffer_allocation")
        guard let b = dev.makeBuffer(length: length, options: option.union(.hazardTrackingModeTracked)) else {
            throw Failure(reason: "buffer_nil_unsupported_no_substitute")
        }
        report["last_buffer"] = ["requested_storage": mode.rawValue, "actual_storage": b.storageMode.rawValue,
            "requested_length": length, "actual_length": b.length]
        try checkpoint("buffer_allocated")
        try need(b.storageMode == mode && b.length == length && b.device.registryID == dev.registryID,
                 "buffer_remapped_or_properties_changed_no_substitute")
        if mode != .private {
            memset(b.contents(), 0xcd, b.length)
            if mode == .managed { b.didModifyRange(NSRange(location: 0, length: b.length)) }
        }
        return b
    }
    func uploaded(_ d: Data) throws -> MTLBuffer {
        let b = try buffer(d.count)
        d.withUnsafeBytes { b.contents().copyMemory(from: $0.baseAddress!, byteCount: $0.count) }; return b
    }
    func command() throws -> MTLCommandBuffer {
        guard let cb = queue.makeCommandBuffer() else { throw Failure(reason: "command_nil") }; return cb
    }
    func finish(_ cb: MTLCommandBuffer, _ name: String) throws {
        cb.label = name; cb.commit(); cb.waitUntilCompleted()
        var c: [String: Any] = ["id": commands.count, "name": name, "status": cb.status.rawValue,
            "completed": cb.status == .completed, "error_present": cb.error != nil,
            "gpu_start": cb.gpuStartTime, "gpu_end": cb.gpuEndTime]
        if let e = cb.error { c["error"] = errorInfo(e) }
        commands.append(c); try checkpoint("command_" + name)
        try need(cb.status == .completed && cb.error == nil, "gpu_command_failed_" + name)
    }
    func blit(_ name: String, _ body: (MTLBlitCommandEncoder) -> Void) throws {
        let cb = try command()
        guard let e = cb.makeBlitCommandEncoder() else { throw Failure(reason: "blit_encoder_nil") }
        body(e); e.endEncoding(); try finish(cb, name)
    }
}
