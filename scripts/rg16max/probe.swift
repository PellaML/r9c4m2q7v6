// Minimal public Metal bridge: upload synthetic bytes, render, and blit raw bytes.
// All arithmetic qualification lives in the separate safe-Rust validator.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ condition: Bool, _ message: String) throws {
    if !condition { throw Failure(message: message) }
}
func sha(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
}
func errorInfo(_ error: Error) -> [String: Any] {
    if let e = error as? Failure { return ["reason": e.message] }
    let e = error as NSError
    // No arbitrary descriptions, userInfo, environment, or URLs in public output.
    return ["reason": "native_api_error", "domain": e.domain, "code": e.code]
}

struct Spec: Decodable {
    let name: String
    let precision: Int
    let mode: String
    let mask: UInt
    let tag: UInt32
    let skip: UInt32
    let negative: Bool
}
struct Manifest: Decodable {
    let schema: Int
    let count: Int
    let width: Int
    let height: Int
    let pitch: Int
    let guardBytes: Int
    let traceGuard: Int
    let captures: [Spec]
}

final class Store {
    let out: URL
    var hashes: [String: String] = [:]
    var bytes = 0
    init(_ out: URL) { self.out = out }
    func save(_ name: String, _ data: Data) throws {
        try need(!name.contains("/") && !name.contains("\\") && !name.hasPrefix("."), "invalid output name")
        try need(hashes[name] == nil && hashes.count < 48, "output name/count limit")
        try need(data.count <= 1_048_576 && bytes + data.count <= 2 * 1024 * 1024, "native output byte limit")
        try data.write(to: out.appendingPathComponent(name), options: .withoutOverwriting)
        hashes[name] = sha(data); bytes += data.count
    }
}

var report: [String: Any] = ["schema": 1, "status": "FAILED_NOT_PASS", "cpu_result_fallback": false]
var commands: [[String: Any]] = []
var observations: [[String: Any]] = []
var receipts = "tag\tname\tstatus\terror\tgpu_start\tgpu_end\tformat\twidth\theight\tmask\tblending\trgb_op\talpha_op\tsrc_rgb\tdst_rgb\tsrc_alpha\tdst_alpha\treturn_bits\n"

func run(_ sourceRoot: URL, _ store: Store) throws {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little-endian host required")
    var sources: [String: String] = [:]
    for name in ["cases.rs", "probe.swift", "probe.metal", "run.sh"] {
        sources[name] = sha(try Data(contentsOf: sourceRoot.appendingPathComponent(name)))
    }
    report["source_sha256"] = sources
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["toolchain_receipt"] = "../run-receipt.txt"
    report["layout"] = [
        "target": "full little-endian raw RG16Float blit buffer, including prefix/suffix and row-stride guards",
        "trace": "64-byte prefix; per pixel uint4: magic XOR tag, row, native return R bits, native return G bits; 64-byte suffix",
        "half_trace": "half2 is converted from the supplied float2 in MSL, then its ushort2 bits are zero-extended to uint2; no host conversion",
        "padding": "host-visible blit stride padding only; opaque private texture allocation padding is not accessible",
        "actual_channels": "RG only; no B/A target words exist; alpha MAX is configured but has no stored alpha output"
    ]
    report["factor_ignorance_measured"] = false
    report["nan_policy"] = "retain every raw target/trace word; no NaN payload, infinity, or zero-sign arithmetic expectation"

    func input(_ name: String) throws -> Data {
        let data = try Data(contentsOf: store.out.appendingPathComponent(name))
        try need(data.count <= 65536, "input byte budget")
        return data
    }
    let manifestData = try input("cases.json")
    let sourceData = try input("source.bin")
    let seedData = try input("seed.bin")
    let m = try JSONDecoder().decode(Manifest.self, from: manifestData)
    try need(m.schema == 1 && m.count == 95 && m.count <= 128 && m.width == 3 && m.height == m.count + 2 &&
             m.pitch == 256 && m.guardBytes == 256 && m.traceGuard == 64 && m.captures.count == 19, "manifest geometry/bounds")
    try need(sourceData.count == m.count * 8 && seedData.count == m.guardBytes * 2 + m.height * m.pitch, "input lengths")
    for (i, s) in m.captures.enumerated() {
        try need(s.tag == UInt32(i + 1) && [16, 32].contains(s.precision) &&
                 ["default", "replace", "max"].contains(s.mode) && [0, 4, 8, 12, 15].contains(s.mask) &&
                 s.name.count <= 40 && s.name.range(of: "^[a-z0-9-]+$", options: .regularExpression) != nil &&
                 s.negative == (i == 18) && s.skip == (s.negative ? 2 : UInt32.max), "capture plan bounds")
    }
    try need(Set(m.captures.map { $0.name }).count == 19, "capture names must be unique")
    report["input_sha256"] = ["cases.json": sha(manifestData), "source.bin": sha(sourceData), "seed.bin": sha(seedData)]
    report["geometry"] = ["width": m.width, "height": m.height, "active_x": 1, "active_y": 1,
        "active_width": 1, "active_height": m.count, "blit_bytes_per_row": m.pitch, "blit_offset": m.guardBytes]
    try need(MTLPixelFormat.rg16Float.rawValue == 65 && MTLColorWriteMask.red.rawValue == 8 &&
             MTLColorWriteMask.green.rawValue == 4 && MTLBlendFactor.one.rawValue == 1 &&
             MTLBlendOperation.max.rawValue == 4 && MTLCommandBufferStatus.completed.rawValue == 4, "public API enum mismatch")

    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(message: "Metal device/queue unavailable; no substitute")
    }
    var families: [String: Bool] = [:]
    let knownFamilies: [(String, MTLGPUFamily)] = [
        ("apple1", .apple1), ("apple2", .apple2), ("apple3", .apple3), ("apple4", .apple4),
        ("apple5", .apple5), ("apple6", .apple6), ("apple7", .apple7),
        ("mac1", .mac1), ("mac2", .mac2), ("common1", .common1), ("common2", .common2), ("common3", .common3)
    ]
    for (name, family) in knownFamilies { families[name] = dev.supportsFamily(family) }
    if #available(macOS 13.0, *) {
        families["apple8"] = dev.supportsFamily(.apple8)
        families["metal3"] = dev.supportsFamily(.metal3)
    }
    if #available(macOS 14.0, *) { families["apple9"] = dev.supportsFamily(.apple9) }
    var deviceInfo: [String: Any] = ["name": dev.name, "registry_id": String(dev.registryID),
        "has_unified_memory": dev.hasUnifiedMemory, "is_low_power": dev.isLowPower,
        "is_removable": dev.isRemovable, "max_buffer_length": dev.maxBufferLength,
        "max_threads_per_threadgroup": [dev.maxThreadsPerThreadgroup.width, dev.maxThreadsPerThreadgroup.height, dev.maxThreadsPerThreadgroup.depth],
        "family_queries": families, "family_queries_exhaustive": false]
    if #available(macOS 14.0, *) { deviceInfo["architecture"] = dev.architecture.name }
    report["device"] = deviceInfo

    let options = MTLCompileOptions()
    options.fastMathEnabled = false
    report["compiler"] = ["fastMathEnabled": options.fastMathEnabled,
        "languageVersion": options.languageVersion.rawValue, "language_selection": "SDK default",
        "compile_route": "MTLDevice.makeLibrary(source:options:)", "swift_flags": ["-swift-version", "5", "-O"]]
    let library = try dev.makeLibrary(source: String(contentsOf: sourceRoot.appendingPathComponent("probe.metal"), encoding: .utf8), options: options)
    report["msl_library_compiled"] = true

    func buffer(_ length: Int) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: length, options: .storageModeShared) else {
            throw Failure(message: "shared buffer allocation failed")
        }
        try need(b.storageMode == .shared && b.length == length, "actual buffer properties mismatch")
        memset(b.contents(), 0xcd, b.length)
        return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let upload = try buffer(seedData.count)
    seedData.withUnsafeBytes { upload.contents().copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let inputs = try buffer(sourceData.count + 128)
    sourceData.withUnsafeBytes { inputs.contents().advanced(by: 64).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let inputsBefore = snapshot(inputs)
    let size = MTLSize(width: m.width, height: m.height, depth: 1)

    func texture() throws -> MTLTexture {
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rg16Float, width: m.width, height: m.height, mipmapped: false)
        d.storageMode = .private
        d.usage = .renderTarget
        d.hazardTrackingMode = .tracked
        guard let t = dev.makeTexture(descriptor: d) else { throw Failure(message: "RG16Float render target unavailable") }
        try need(t.pixelFormat == .rg16Float && t.storageMode == .private && t.usage == .renderTarget &&
                 t.textureType == .type2D && t.width == m.width && t.height == m.height && t.depth == 1 &&
                 t.mipmapLevelCount == 1 && t.arrayLength == 1 && t.sampleCount == 1 &&
                 t.buffer == nil && !t.isFramebufferOnly && t.hazardTrackingMode == .tracked, "actual target properties mismatch")
        return t
    }
    func textureInfo(_ t: MTLTexture) -> [String: Any] {
        ["format": t.pixelFormat.rawValue, "storage": t.storageMode.rawValue, "usage": t.usage.rawValue,
         "type": t.textureType.rawValue, "width": t.width, "height": t.height, "depth": t.depth,
         "mips": t.mipmapLevelCount, "array_length": t.arrayLength, "samples": t.sampleCount,
         "buffer_backed": t.buffer != nil, "framebuffer_only": t.isFramebufferOnly,
         "hazard_tracking_mode": t.hazardTrackingMode.rawValue]
    }
    func uploadTexture(_ cb: MTLCommandBuffer, _ t: MTLTexture) throws {
        guard let b = cb.makeBlitCommandEncoder() else { throw Failure(message: "upload blit encoder unavailable") }
        b.copy(from: upload, sourceOffset: m.guardBytes, sourceBytesPerRow: m.pitch, sourceBytesPerImage: m.pitch * m.height,
               sourceSize: size, to: t, destinationSlice: 0, destinationLevel: 0, destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
        b.endEncoding()
    }
    func readTexture(_ cb: MTLCommandBuffer, _ t: MTLTexture, _ result: MTLBuffer) throws {
        guard let b = cb.makeBlitCommandEncoder() else { throw Failure(message: "readback blit encoder unavailable") }
        b.copy(from: t, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0), sourceSize: size,
               to: result, destinationOffset: m.guardBytes, destinationBytesPerRow: m.pitch, destinationBytesPerImage: m.pitch * m.height)
        b.endEncoding()
    }
    func finish(_ cb: MTLCommandBuffer, _ name: String, _ tag: UInt32, _ t: MTLTexture, _ state: [UInt]) throws {
        cb.label = name
        cb.commit()
        cb.waitUntilCompleted()
        var entry: [String: Any] = ["name": name, "tag": tag, "status": cb.status.rawValue,
            "completed": cb.status == .completed, "error_present": cb.error != nil,
            "gpu_start": cb.gpuStartTime, "gpu_end": cb.gpuEndTime,
            "timing_note": "zero timestamps are retained as unavailable, not invented"]
        if let error = cb.error { entry["error"] = errorInfo(error) }
        commands.append(entry)
        let values: [String] = [String(tag), name, String(cb.status.rawValue), cb.error == nil ? "0" : "1",
            String(cb.gpuStartTime), String(cb.gpuEndTime), String(t.pixelFormat.rawValue), String(t.width), String(t.height)] + state.map(String.init)
        receipts += values.joined(separator: "\t") + "\n"
        try need(cb.status == .completed && cb.error == nil, "GPU command failed: " + name)
    }
    let baseline = try texture()
    let baselineReadback = try buffer(seedData.count)
    guard let baselineCommand = queue.makeCommandBuffer() else { throw Failure(message: "baseline command unavailable") }
    try uploadTexture(baselineCommand, baseline)
    try readTexture(baselineCommand, baseline, baselineReadback)
    try finish(baselineCommand, "baseline", 0, baseline, Array(repeating: 0, count: 9))
    let baselineData = snapshot(baselineReadback)
    try store.save("baseline-target.bin", baselineData)
    try need(baselineData == seedData, "raw GPU seed upload/readback changed bytes")
    report["baseline_texture"] = textureInfo(baseline)
    report["raw_seed_roundtrip_exact"] = true

    for s in m.captures {
        var obs: [String: Any] = ["name": s.name, "tag": s.tag, "status": "FAILED_NOT_PASS",
            "return_type": s.precision == 32 ? "float2" : "half2", "return_bits": s.precision,
            "mode": s.mode, "negative": s.negative, "skip_row": s.skip]
        do {
            let d = MTLRenderPipelineDescriptor()
            d.label = s.name
            d.rasterSampleCount = 1
            d.isAlphaToCoverageEnabled = false
            d.isAlphaToOneEnabled = false
            d.isRasterizationEnabled = true
            guard let vertex = library.makeFunction(name: "full_triangle") else { throw Failure(message: "vertex missing") }
            let fragmentName = (s.mode == "default" ? "control_" : "probe_") + (s.precision == 32 ? "float" : "half")
            guard let fragment = library.makeFunction(name: fragmentName) else { throw Failure(message: "fragment missing") }
            d.vertexFunction = vertex; d.fragmentFunction = fragment
            let a = d.colorAttachments[0]!
            a.pixelFormat = .rg16Float
            if s.mode != "default" {
                a.isBlendingEnabled = s.mode == "max"
                a.sourceRGBBlendFactor = .one; a.destinationRGBBlendFactor = .one
                a.sourceAlphaBlendFactor = .one; a.destinationAlphaBlendFactor = .one
                a.rgbBlendOperation = .max; a.alphaBlendOperation = .max
                a.writeMask = MTLColorWriteMask(rawValue: s.mask)
            }
            // Default control intentionally leaves every blend property/mask untouched.
            let state: [UInt] = [a.writeMask.rawValue, a.isBlendingEnabled ? 1 : 0,
                a.rgbBlendOperation.rawValue, a.alphaBlendOperation.rawValue,
                a.sourceRGBBlendFactor.rawValue, a.destinationRGBBlendFactor.rawValue,
                a.sourceAlphaBlendFactor.rawValue, a.destinationAlphaBlendFactor.rawValue, UInt(s.precision)]
            obs["pipeline_descriptor_at_creation"] = ["pixel_format": a.pixelFormat.rawValue,
                "write_mask": a.writeMask.rawValue, "blending_enabled": a.isBlendingEnabled,
                "rgb_operation": a.rgbBlendOperation.rawValue, "alpha_operation": a.alphaBlendOperation.rawValue,
                "src_rgb": a.sourceRGBBlendFactor.rawValue, "dst_rgb": a.destinationRGBBlendFactor.rawValue,
                "src_alpha": a.sourceAlphaBlendFactor.rawValue, "dst_alpha": a.destinationAlphaBlendFactor.rawValue,
                "sample_count": d.rasterSampleCount, "fragment_entry": fragmentName,
                "state_note": "descriptor queried immediately before compilation; not opaque pipeline introspection"]
            let pso = try dev.makeRenderPipelineState(descriptor: d)
            obs["pipeline_compiled"] = true
            let target = try texture()
            obs["actual_texture"] = textureInfo(target)
            let raw = try buffer(seedData.count)
            let trace = try buffer(m.traceGuard * 2 + m.count * 16)
            guard let cb = queue.makeCommandBuffer() else { throw Failure(message: "render command unavailable") }
            try uploadTexture(cb, target)
            let pass = MTLRenderPassDescriptor()
            pass.colorAttachments[0].texture = target
            pass.colorAttachments[0].loadAction = .load
            pass.colorAttachments[0].storeAction = .store
            guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(message: "render encoder unavailable") }
            e.setRenderPipelineState(pso)
            e.setCullMode(.none)
            e.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(m.width), height: Double(m.height), znear: 0, zfar: 1))
            e.setScissorRect(MTLScissorRect(x: 1, y: 1, width: 1, height: m.count))
            e.setFragmentBuffer(inputs, offset: 64, index: 0)
            e.setFragmentBuffer(trace, offset: m.traceGuard, index: 1)
            var params = SIMD4<UInt32>(UInt32(m.count), s.tag, s.skip, 0)
            e.setFragmentBytes(&params, length: 16, index: 2)
            e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            e.endEncoding()
            try readTexture(cb, target, raw)
            try finish(cb, s.name, s.tag, target, state)
            // These exact CPU-visible copies happen only after real GPU completion.
            // No lane repair, NaN canonicalization, half conversion, or expected output write.
            try store.save(s.name + "-target.bin", snapshot(raw))
            try store.save(s.name + "-trace.bin", snapshot(trace))
            try need(snapshot(upload) == seedData && snapshot(inputs) == inputsBefore, "GPU changed input data/guards")
            obs["input_buffers_unchanged"] = true
            obs["status"] = "CAPTURED_PENDING_ORDINARY_VALIDATOR"
        } catch {
            obs["error"] = errorInfo(error)
            observations.append(obs)
            throw error
        }
        observations.append(obs)
    }
    try need(commands.count == 20 && observations.count == 19, "capture coverage incomplete")
    report["status"] = "CAPTURES_COMPLETE_PENDING_ORDINARY_VALIDATOR"
}

guard CommandLine.arguments.count == 3 else { exit(2) }
let store = Store(URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true))
var code: Int32 = 0
do { try run(URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true), store) }
catch { report["failure"] = errorInfo(error); code = 3 }
report["commands"] = commands
report["observations"] = observations
report["exit_code"] = code
do {
    try store.save("receipts.tsv", Data(receipts.utf8))
    report["raw_output_sha256"] = store.hashes
    report["raw_output_bytes"] = store.bytes
    let json = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    try store.save("gpu.json", json)
} catch { code = 4 }
print("native_capture_exit=\(code)")
exit(code)
