// Minimal public Metal bridge adapted from scripts/rg16max/probe.swift.
// Synthetic uploads + real draws/blits only. Rust owns checking and hypotheses.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let message: String }
func need(_ ok: Bool, _ message: String) throws { if !ok { throw Failure(message: message) } }
func sha(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
func errorInfo(_ error: Error) -> [String: Any] {
    if let e = error as? Failure { return ["reason": e.message] }
    let e = error as NSError
    // Never include arbitrary descriptions, userInfo, shader binaries, URLs or addresses.
    return ["reason": "native_api_error", "domain": e.domain, "code": e.code]
}
struct Spec: Decodable {
    let name: String; let precision: Int; let mode: String; let mask: UInt
    let tag: UInt32; let skip: UInt32; let negative: Bool
}
struct Manifest: Decodable {
    let schema: Int; let count: Int; let width: Int; let height: Int; let pitch: Int
    let guardBytes: Int; let traceGuard: Int; let traceStride: Int; let captures: [Spec]
}
final class Store {
    let out: URL; var hashes: [String: String] = [:]; var bytes = 0
    init(_ out: URL) { self.out = out }
    func save(_ name: String, _ data: Data) throws {
        try need(!name.contains("/") && !name.contains("\\") && !name.hasPrefix("."), "output name")
        try need(hashes[name] == nil && hashes.count < 80, "native output count")
        try need(data.count <= 1_048_576 && bytes + data.count <= 3 * 1024 * 1024, "native output bytes")
        try data.write(to: out.appendingPathComponent(name), options: .withoutOverwriting)
        hashes[name] = sha(data); bytes += data.count
    }
}
var report: [String: Any] = ["schema": 1, "status": "FAILED_NOT_PASS", "cpu_result_fallback": false]
var commands: [[String: Any]] = []
var observations: [[String: Any]] = []
var receipts = "tag\tname\tstatus\terror\tgpu_start\tgpu_end\tformat\twidth\theight\tmask\tblending\trgb_op\talpha_op\tsrc_rgb\tdst_rgb\tsrc_alpha\tdst_alpha\treturn_bits\tpipeline_format\tsamples\talpha_to_coverage\talpha_to_one\trasterization\tload_action\tstore_action\tdraws\tvertices\tpso_created\n"

func run(_ sourceRoot: URL, _ store: Store) throws {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little-endian host required")
    var sources: [String: String] = [:]
    for name in ["cases.rs", "probe.swift", "probe.metal", "run.sh"] {
        sources[name] = sha(try Data(contentsOf: sourceRoot.appendingPathComponent(name)))
    }
    report["source_sha256"] = sources
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["layout"] = [
        "target": "raw little-endian RGBA16Float; 256-byte prefix/suffix; 256-byte rows; three pixels wide",
        "trace": "64-byte prefix/suffix; per active pixel uint4 header then uint4 return bits; header magic XOR tag, row, precision, bitwise-not row",
        "half_input": "raw ushort4 bitcast to half4; no float-to-half conversion in shader",
        "float_input": "raw uint4 bitcast to float4; no half conversion in shader",
        "upload": "each fresh private target is blitted back BEFORE its draw, in the same command buffer",
        "padding_scope": "host-visible copy stride padding only, not opaque texture allocation padding"
    ]
    report["nonfinite_policy"] = "all raw bits retained; no NaN payload, infinity or zero-sign MIN rule presumed"
    report["factor_ignorance_measured"] = false
    func input(_ name: String) throws -> Data {
        let d = try Data(contentsOf: store.out.appendingPathComponent(name))
        try need(d.count <= 65536, "input byte limit"); return d
    }
    let manifestData = try input("cases.json")
    let floatData = try input("source-float.bin")
    let halfData = try input("source-half.bin")
    let seedData = try input("seed.bin")
    let m = try JSONDecoder().decode(Manifest.self, from: manifestData)
    try need(m.schema == 1 && m.count == 164 && m.width == 3 && m.height == m.count + 2 &&
             m.pitch == 256 && m.guardBytes == 256 && m.traceGuard == 64 && m.traceStride == 32 && m.captures.count == 21, "manifest geometry")
    try need(floatData.count == m.count * 16 && halfData.count == m.count * 8 &&
             seedData.count == m.guardBytes * 2 + m.height * m.pitch, "input lengths")
    let masks: [UInt] = [0, 8, 4, 2, 1, 5, 10, 15]
    for (i, s) in m.captures.enumerated() {
        let negative = i == 20
        let precision = i < 10 || negative ? 32 : 16
        let slot = i % 10
        let mode = negative || slot >= 2 ? "min" : (slot == 0 ? "default" : "replace")
        let mask: UInt = negative || slot < 2 ? 15 : masks[slot - 2]
        let name = negative ? "f32-negative-skip" : "f\(precision)-\(mode)" + (mode == "min" ? "-\(mask)" : "")
        try need(s.tag == UInt32(i + 1) && s.precision == precision && s.mode == mode && s.mask == mask &&
                 s.name == name && s.negative == negative && s.skip == (negative ? 2 : UInt32.max), "exact capture plan")
    }
    report["input_sha256"] = ["cases.json": sha(manifestData), "source-float.bin": sha(floatData),
                              "source-half.bin": sha(halfData), "seed.bin": sha(seedData)]
    // Actual imported SDK enum getters, recorded before assertions. No private enum names.
    let enums: [(String, UInt)] = [
        ("rgba16Float", MTLPixelFormat.rgba16Float.rawValue), ("min", MTLBlendOperation.min.rawValue),
        ("one", MTLBlendFactor.one.rawValue), ("red", MTLColorWriteMask.red.rawValue),
        ("green", MTLColorWriteMask.green.rawValue), ("blue", MTLColorWriteMask.blue.rawValue),
        ("alpha", MTLColorWriteMask.alpha.rawValue), ("all", MTLColorWriteMask.all.rawValue),
        ("completed", MTLCommandBufferStatus.completed.rawValue), ("load", MTLLoadAction.load.rawValue),
        ("store", MTLStoreAction.store.rawValue)
    ]
    let enumText = "name\tvalue\n" + enums.map { "\($0.0)\t\($0.1)\n" }.joined()
    try store.save("sdk-enums.tsv", Data(enumText.utf8))
    try need(enums.map { $0.1 } == [115, 3, 1, 8, 4, 2, 1, 15, 4, 1, 1], "actual SDK enum mismatch")
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(message: "Metal device or queue unavailable; no substitute")
    }
    // Public capabilities only. No registry identifier, address or captured application name.
    report["device"] = ["public_name": dev.name, "has_unified_memory": dev.hasUnifiedMemory,
        "mac2": dev.supportsFamily(.mac2), "apple7": dev.supportsFamily(.apple7),
        "apple8": dev.supportsFamily(.apple8), "apple9": dev.supportsFamily(.apple9)]
    let options = MTLCompileOptions()
    options.fastMathEnabled = false
    options.languageVersion = .version2_4
    let compilerText = "property\tvalue\nfastMathEnabled\t\(options.fastMathEnabled ? 1 : 0)\nlanguageVersion\t\(options.languageVersion.rawValue)\nbinary32DenormControl\tnot-explicitly-set\nprofileScope\tthis-profile-only\n"
    try store.save("compiler.tsv", Data(compilerText.utf8))
    try need(!options.fastMathEnabled && options.languageVersion == .version2_4, "compiler option getters")
    report["compiler"] = ["fastMathEnabled": options.fastMathEnabled,
        "languageVersion": options.languageVersion.rawValue, "compile_route": "makeLibrary(source:options:)",
        "language_selection": "explicit public MSL 2.4 syntax; not a claim of MSL 4.1 fixed-function behavior",
        "binary32_denorm_control": "not explicitly set; no equivalence to any other compiler profile asserted",
        "half_return_bits": "actual raw half4 words, widened losslessly to uint4 trace immediately before return"]
    let library = try dev.makeLibrary(source: String(contentsOf: sourceRoot.appendingPathComponent("probe.metal"), encoding: .utf8), options: options)
    report["msl_library_compiled"] = true

    func buffer(_ length: Int) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: length, options: .storageModeShared) else { throw Failure(message: "shared allocation") }
        try need(b.storageMode == .shared && b.length == length, "buffer getters")
        memset(b.contents(), 0xcd, b.length); return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    func sourceBuffer(_ data: Data) throws -> MTLBuffer {
        let b = try buffer(data.count + m.traceGuard * 2)
        data.withUnsafeBytes { b.contents().advanced(by: m.traceGuard).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
        return b
    }
    let upload = try buffer(seedData.count)
    seedData.withUnsafeBytes { upload.contents().copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let floatInputs = try sourceBuffer(floatData); let halfInputs = try sourceBuffer(halfData)
    let floatBefore = snapshot(floatInputs); let halfBefore = snapshot(halfInputs)
    let size = MTLSize(width: m.width, height: m.height, depth: 1)
    func texture() throws -> MTLTexture {
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba16Float, width: m.width, height: m.height, mipmapped: false)
        d.storageMode = .private; d.usage = .renderTarget; d.hazardTrackingMode = .tracked
        guard let t = dev.makeTexture(descriptor: d) else { throw Failure(message: "RGBA16Float render target unavailable") }
        try need(t.pixelFormat == .rgba16Float && t.storageMode == .private && t.usage == .renderTarget &&
                 t.textureType == .type2D && t.width == m.width && t.height == m.height && t.depth == 1 &&
                 t.mipmapLevelCount == 1 && t.arrayLength == 1 && t.sampleCount == 1 &&
                 t.buffer == nil && !t.isFramebufferOnly && t.hazardTrackingMode == .tracked, "actual texture getters")
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
        guard let b = cb.makeBlitCommandEncoder() else { throw Failure(message: "upload encoder unavailable") }
        b.copy(from: upload, sourceOffset: m.guardBytes, sourceBytesPerRow: m.pitch, sourceBytesPerImage: m.pitch * m.height,
               sourceSize: size, to: t, destinationSlice: 0, destinationLevel: 0, destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0))
        b.endEncoding()
    }
    func readTexture(_ cb: MTLCommandBuffer, _ t: MTLTexture, _ result: MTLBuffer) throws {
        guard let b = cb.makeBlitCommandEncoder() else { throw Failure(message: "readback encoder unavailable") }
        b.copy(from: t, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0), sourceSize: size,
               to: result, destinationOffset: m.guardBytes, destinationBytesPerRow: m.pitch, destinationBytesPerImage: m.pitch * m.height)
        b.endEncoding()
    }
    func finish(_ cb: MTLCommandBuffer, _ name: String, _ tag: UInt32, _ t: MTLTexture, _ state: [UInt]) throws {
        try need(commands.count < 22 && state.count == 19, "command budget or receipt shape")
        cb.label = name; cb.commit(); cb.waitUntilCompleted()
        var entry: [String: Any] = ["name": name, "tag": tag, "status": cb.status.rawValue,
            "completed": cb.status == .completed, "error_present": cb.error != nil,
            "gpu_start": cb.gpuStartTime, "gpu_end": cb.gpuEndTime,
            "timing_note": "zero timestamps mean unavailable, not invented"]
        if let error = cb.error { entry["error"] = errorInfo(error) }
        commands.append(entry)
        let values = [String(tag), name, String(cb.status.rawValue), cb.error == nil ? "0" : "1",
            String(cb.gpuStartTime), String(cb.gpuEndTime), String(t.pixelFormat.rawValue), String(t.width), String(t.height)] + state.map(String.init)
        receipts += values.joined(separator: "\t") + "\n"
        try need(cb.status == .completed && cb.error == nil, "GPU command failed")
        try need(snapshot(upload) == seedData && snapshot(floatInputs) == floatBefore && snapshot(halfInputs) == halfBefore, "input contents or guards changed")
    }
    let baseline = try texture(); let baselineReadback = try buffer(seedData.count)
    guard let bc = queue.makeCommandBuffer() else { throw Failure(message: "baseline command unavailable") }
    try uploadTexture(bc, baseline); try readTexture(bc, baseline, baselineReadback)
    try finish(bc, "baseline", 0, baseline, Array(repeating: 0, count: 19))
    let baselineData = snapshot(baselineReadback)
    try store.save("baseline-target.bin", baselineData)
    try need(baselineData == seedData, "raw GPU seed roundtrip changed bytes")
    report["baseline_texture"] = textureInfo(baseline)

    for s in m.captures {
        var obs: [String: Any] = ["name": s.name, "tag": s.tag, "status": "FAILED_NOT_PASS",
            "return_type": s.precision == 32 ? "float4" : "half4", "mode": s.mode,
            "negative": s.negative, "skip_row": s.skip]
        do {
            let d = MTLRenderPipelineDescriptor()
            d.label = s.name; d.rasterSampleCount = 1; d.isAlphaToCoverageEnabled = false
            d.isAlphaToOneEnabled = false; d.isRasterizationEnabled = true
            guard let vertex = library.makeFunction(name: "full_triangle") else { throw Failure(message: "vertex missing") }
            let fragmentName = (s.mode == "default" ? "control_" : "probe_") + (s.precision == 32 ? "float" : "half")
            guard let fragment = library.makeFunction(name: fragmentName) else { throw Failure(message: "fragment missing") }
            d.vertexFunction = vertex; d.fragmentFunction = fragment
            let a = d.colorAttachments[0]!
            a.pixelFormat = .rgba16Float
            if s.mode != "default" {
                a.isBlendingEnabled = s.mode == "min"
                a.sourceRGBBlendFactor = .one; a.destinationRGBBlendFactor = .one
                a.sourceAlphaBlendFactor = .one; a.destinationAlphaBlendFactor = .one
                a.rgbBlendOperation = .min; a.alphaBlendOperation = .min
                a.writeMask = MTLColorWriteMask(rawValue: s.mask)
            }
            // These are actual descriptor getters at creation, NOT opaque PSO introspection.
            let blendState: [UInt] = [a.writeMask.rawValue, a.isBlendingEnabled ? 1 : 0,
                a.rgbBlendOperation.rawValue, a.alphaBlendOperation.rawValue,
                a.sourceRGBBlendFactor.rawValue, a.destinationRGBBlendFactor.rawValue,
                a.sourceAlphaBlendFactor.rawValue, a.destinationAlphaBlendFactor.rawValue, UInt(s.precision)]
            let want: [UInt] = s.mode == "default" ? [15, 0, 0, 0, 1, 0, 1, 0, UInt(s.precision)] :
                [s.mask, s.mode == "min" ? 1 : 0, 3, 3, 1, 1, 1, 1, UInt(s.precision)]
            obs["descriptor_getters"] = ["format": a.pixelFormat.rawValue, "blend_fields": blendState,
                "samples": d.rasterSampleCount, "alpha_to_coverage": d.isAlphaToCoverageEnabled,
                "alpha_to_one": d.isAlphaToOneEnabled, "rasterization": d.isRasterizationEnabled,
                "vertex_entry": vertex.name, "fragment_entry": fragment.name]
            try need(blendState == want && a.pixelFormat.rawValue == 115 && d.rasterSampleCount == 1 &&
                     !d.isAlphaToCoverageEnabled && !d.isAlphaToOneEnabled && d.isRasterizationEnabled, "pipeline descriptor getter mismatch")
            let pso = try dev.makeRenderPipelineState(descriptor: d)
            obs["pipeline_compiled"] = true
            let target = try texture(); obs["actual_texture"] = textureInfo(target)
            let uploaded = try buffer(seedData.count); let raw = try buffer(seedData.count)
            let trace = try buffer(m.traceGuard * 2 + m.count * m.traceStride)
            guard let cb = queue.makeCommandBuffer() else { throw Failure(message: "render command unavailable") }
            try uploadTexture(cb, target)
            try readTexture(cb, target, uploaded) // independent pre-draw readback for EVERY capture
            let pass = MTLRenderPassDescriptor()
            pass.colorAttachments[0].texture = target
            pass.colorAttachments[0].loadAction = .load; pass.colorAttachments[0].storeAction = .store
            try need(pass.colorAttachments[0].loadAction == .load && pass.colorAttachments[0].storeAction == .store, "pass getters")
            guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(message: "render encoder unavailable") }
            e.setRenderPipelineState(pso); e.setCullMode(.none)
            e.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(m.width), height: Double(m.height), znear: 0, zfar: 1))
            e.setScissorRect(MTLScissorRect(x: 1, y: 1, width: 1, height: m.count))
            e.setFragmentBuffer(s.precision == 32 ? floatInputs : halfInputs, offset: m.traceGuard, index: 0)
            e.setFragmentBuffer(trace, offset: m.traceGuard, index: 1)
            var params = SIMD4<UInt32>(UInt32(m.count), s.tag, s.skip, UInt32(s.precision))
            e.setFragmentBytes(&params, length: 16, index: 2)
            e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
            e.endEncoding(); try readTexture(cb, target, raw)
            let state = blendState + [a.pixelFormat.rawValue, UInt(d.rasterSampleCount), d.isAlphaToCoverageEnabled ? 1 : 0,
                d.isAlphaToOneEnabled ? 1 : 0, d.isRasterizationEnabled ? 1 : 0,
                pass.colorAttachments[0].loadAction.rawValue, pass.colorAttachments[0].storeAction.rawValue, 1, 3, 1]
            try finish(cb, s.name, s.tag, target, state)
            // Copies only after GPU completion: no repair, pack, canonicalize or expected-output path.
            try store.save(s.name + "-upload.bin", snapshot(uploaded))
            try store.save(s.name + "-target.bin", snapshot(raw))
            try store.save(s.name + "-trace.bin", snapshot(trace))
            try need(snapshot(uploaded) == seedData, "per-capture upload readback changed bytes")
            obs["status"] = "CAPTURED_PENDING_RUST_VALIDATOR"
        } catch { obs["error"] = errorInfo(error); observations.append(obs); throw error }
        observations.append(obs)
    }
    try store.save("source-float-after.bin", snapshot(floatInputs))
    try store.save("source-half-after.bin", snapshot(halfInputs))
    try store.save("seed-after.bin", snapshot(upload))
    try need(commands.count == 22 && observations.count == 21, "capture coverage incomplete")
    try store.save("complete.txt", Data("schema=1\nstatus=0\ncommands=22\ncaptures=21\n".utf8))
    report["status"] = "CAPTURES_COMPLETE_PENDING_RUST_VALIDATOR"
}
guard CommandLine.arguments.count == 3 else { exit(2) }
let store = Store(URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true))
var code: Int32 = 0
do { try run(URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true), store) }
catch { report["failure"] = errorInfo(error); code = 3 }
report["commands"] = commands; report["observations"] = observations; report["exit_code"] = code
do {
    try store.save("receipts.tsv", Data(receipts.utf8))
    report["raw_output_sha256"] = store.hashes; report["raw_output_bytes"] = store.bytes
    try store.save("gpu.json", JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]))
} catch { code = 4 }
print("native_capture_exit=\(code)")
exit(code)
