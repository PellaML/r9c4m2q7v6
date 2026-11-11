// Minimal existing-public-harness Metal bridge: raw runtime input, fragment
// sidecar words, guarded completed readback. No CPU clamp or NaN endpoint.
import Foundation
import Metal
import CryptoKit
struct Failure: Error { let message: String }
func need(_ ok: Bool, _ message: String) throws { if !ok { throw Failure(message: message) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func errorInfo(_ error: Error) -> [String: Any] {
    if let e = error as? Failure { return ["reason": e.message] }
    let e = error as NSError
    return ["reason": "native_api_error", "domain": e.domain, "code": e.code]
}
struct Plan: Decodable {
    let schema: Int; let count: Int; let width: Int; let height: Int
    let input_stride: Int; let trace_stride: Int
    let guardBytes: Int
    enum CodingKeys: String, CodingKey { case schema, count, width, height, input_stride, trace_stride; case guardBytes = "guard" }
}
final class Store {
    let out: URL; var hashes: [String: String] = [:]; var bytes = 0
    init(_ out: URL) { self.out = out }
    func save(_ name: String, _ data: Data) throws {
        try need(!name.contains("/") && !name.contains("\\") && !name.hasPrefix("."), "output name")
        try need(hashes[name] == nil && hashes.count < 16, "native output count")
        try need(data.count <= 1_048_576 && bytes + data.count <= 3 * 1024 * 1024, "native output budget")
        try data.write(to: out.appendingPathComponent(name), options: .withoutOverwriting)
        hashes[name] = sha(data); bytes += data.count
    }
}
var report: [String: Any] = ["schema": 1, "status": "FAILED_NOT_PASS", "cpu_output_fallback": false]
var commands: [[String: Any]] = []
var receipts = "tag\tcapture\tprofile\tstatus\terror\tgpu_start\tgpu_end\tformat\twidth\theight\tsamples\twrite_mask\tblending\trasterization\talpha_to_coverage\talpha_to_one\tload\tstore\tdraws\tvertices\tpso_created\tcases\tinput_bytes\ttrace_bytes\n"
var compilerReceipts = "profile\tfast_math\tmath_mode\tmath_functions\tlanguage\toptimization\tpreserve_invariance\tmode_matches\tfunctions_match\tlanguage_match\tdenorm_override\tdefault_fast\tdefault_mode\tdefault_functions\tdefault_language\n"

func run(_ sourceRoot: URL, _ store: Store) throws {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little-endian host")
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["layout"] = [
        "input": "64-byte guards; each 16-byte record: four raw ushort a,b,lower,upper; uint route,bound",
        "trace": "64-byte guards; each 32-byte record: uint2 encoded marker,row; ushort4 raw operands; ushort4 actual half input,dynamic clamp,literal clamp 0/1,separate saturate; ushort4 bitwise inverses",
        "marker": "0x68636c00 XOR (tag<<16) XOR (route<<8) XOR bound; bounded tag/route/bound occupy distinct bit fields",
        "numeric_path": "scalar half arithmetic into raw ushort sidecars; dummy RGBA8 sink is never read",
        "profile_scope": "explicit public compile options; no equivalence to private IR modes or all profiles",
        "composition_scope": "owned synthetic half division inputs only; intermediate class is observed, not assumed",
        "nan_policy": "retain every result word; do not assume lower, upper, saturate, finite, or canonical NaN output"
    ]
    var sourceHashes: [String: String] = [:]
    for name in ["cases.rs", "probe.swift", "probe.metal", "run.sh"] {
        sourceHashes[name] = sha(try Data(contentsOf: sourceRoot.appendingPathComponent(name)))
    }
    report["source_sha256"] = sourceHashes
    let planData = try Data(contentsOf: store.out.appendingPathComponent("cases.json"))
    let input = try Data(contentsOf: store.out.appendingPathComponent("inputs.bin"))
    try need(planData.count <= 16384 && input.count <= 1_048_576, "input size")
    let p = try JSONDecoder().decode(Plan.self, from: planData)
    try need(p.schema == 1 && p.count == 24848 && p.width == 64 && p.height == 389 && p.guardBytes == 64 &&
             p.input_stride == 16 && p.trace_stride == 32 && input.count == p.count * p.input_stride, "exact input geometry")
    report["input_sha256"] = ["cases.json": sha(planData), "inputs.bin": sha(input)]
    // Actual SDK enums, not guessed private selector numbers. Rust joins math
    // option getters to these same public enum getters.
    let sdk: [(String, Int)] = [
        ("rgba8Unorm", Int(MTLPixelFormat.rgba8Unorm.rawValue)),
        ("completed", Int(MTLCommandBufferStatus.completed.rawValue)),
        ("language24", Int(MTLLanguageVersion.version2_4.rawValue)),
        ("load_dontcare", Int(MTLLoadAction.dontCare.rawValue)),
        ("store_dontcare", Int(MTLStoreAction.dontCare.rawValue)),
        ("mode_fast", Int(MTLMathMode.fast.rawValue)), ("mode_safe", Int(MTLMathMode.safe.rawValue)),
        ("functions_fast", Int(MTLMathFloatingPointFunctions.fast.rawValue)),
        ("functions_precise", Int(MTLMathFloatingPointFunctions.precise.rawValue))
    ]
    try store.save("sdk-enums.tsv", Data(("symbol\tvalue\n" + sdk.map { "\($0.0)\t\($0.1)\n" }.joined()).utf8))
    try need(sdk.prefix(5).map { $0.1 } == [70, 4, 131076, 0, 0] &&
             MTLMathMode.fast != .safe && MTLMathFloatingPointFunctions.fast != .precise, "SDK enum mismatch")
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(message: "Metal device or queue unavailable; no substitute")
    }
    report["device"] = ["public_name": dev.name, "unified_memory": dev.hasUnifiedMemory,
        "mac2": dev.supportsFamily(.mac2), "apple7": dev.supportsFamily(.apple7), "apple9": dev.supportsFamily(.apple9)]
    func buffer(_ size: Int) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: size, options: .storageModeShared) else { throw Failure(message: "shared buffer allocation") }
        try need(b.length == size && b.storageMode == .shared, "buffer getters")
        memset(b.contents(), 0xcd, b.length); return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let inputs = try buffer(input.count + p.guardBytes * 2)
    input.withUnsafeBytes { inputs.contents().advanced(by: p.guardBytes).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let before = snapshot(inputs)
    let msl = try String(contentsOf: sourceRoot.appendingPathComponent("probe.metal"), encoding: .utf8)
    try need(msl.utf8.count <= 32768, "shader source size")
    var pipelines: [String: MTLRenderPipelineState] = [:]
    var descriptors: [String: MTLRenderPipelineDescriptor] = [:]
    var compilerInfo: [[String: Any]] = []
    for profile in ["fast", "precise"] {
        let fast = profile == "fast"
        let options = MTLCompileOptions()
        let defaults = [options.fastMathEnabled ? 1 : 0, Int(options.mathMode.rawValue),
                        Int(options.mathFloatingPointFunctions.rawValue), Int(options.languageVersion.rawValue)]
        // Same explicit public fast configuration as the existing half-fetch
        // harness; the second library is a bounded safe/precise control only.
        options.languageVersion = .version2_4
        options.fastMathEnabled = fast
        options.mathMode = fast ? .fast : .safe
        options.mathFloatingPointFunctions = fast ? .fast : .precise
        let modeMatch = options.mathMode == (fast ? MTLMathMode.fast : .safe)
        let functionsMatch = options.mathFloatingPointFunctions == (fast ? MTLMathFloatingPointFunctions.fast : .precise)
        let values = [options.fastMathEnabled ? 1 : 0, Int(options.mathMode.rawValue),
            Int(options.mathFloatingPointFunctions.rawValue), Int(options.languageVersion.rawValue),
            Int(options.optimizationLevel.rawValue), options.preserveInvariance ? 1 : 0,
            modeMatch ? 1 : 0, functionsMatch ? 1 : 0, options.languageVersion == .version2_4 ? 1 : 0, 0] + defaults
        compilerReceipts += ([profile] + values.map(String.init)).joined(separator: "\t") + "\n"
        compilerInfo.append(["profile": profile, "getters": values,
            "route": "MTLDevice.makeLibrary(source:options:)", "binary32_denorm_override": "not requested",
            "nonfinite_fast_math_note": "observations only; fast optimizations may assume no NaN/Inf"])
        report["compiler"] = compilerInfo
        try need(options.fastMathEnabled == fast && modeMatch && functionsMatch && options.languageVersion == .version2_4, "compiler option getters")
        let library = try dev.makeLibrary(source: msl, options: options)
        let d = MTLRenderPipelineDescriptor()
        d.vertexFunction = library.makeFunction(name: "full_triangle")
        d.fragmentFunction = library.makeFunction(name: "observe_half")
        try need(d.vertexFunction != nil && d.fragmentFunction != nil, "shader entry missing")
        d.rasterSampleCount = 1; d.isRasterizationEnabled = true
        d.isAlphaToCoverageEnabled = false; d.isAlphaToOneEnabled = false
        d.colorAttachments[0].pixelFormat = .rgba8Unorm
        d.colorAttachments[0].isBlendingEnabled = false; d.colorAttachments[0].writeMask = .all
        d.depthAttachmentPixelFormat = .invalid; d.stencilAttachmentPixelFormat = .invalid
        pipelines[profile] = try dev.makeRenderPipelineState(descriptor: d)
        descriptors[profile] = d
    }
    var observations: [[String: Any]] = []
    for (index, name) in ["fast", "precise", "fast-missing"].enumerated() {
        let profile = name == "precise" ? "precise" : "fast"
        let tag = UInt32(index + 1); let skip = name == "fast-missing" ? UInt32(0) : UInt32.max
        guard let pipeline = pipelines[profile], let d = descriptors[profile] else { throw Failure(message: "profile pipeline missing") }
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba8Unorm, width: p.width, height: p.height, mipmapped: false)
        td.storageMode = .private; td.usage = .renderTarget; td.hazardTrackingMode = .tracked
        guard let sink = dev.makeTexture(descriptor: td) else { throw Failure(message: "dummy target allocation") }
        try need(sink.pixelFormat == .rgba8Unorm && sink.width == p.width && sink.height == p.height && sink.depth == 1 &&
                 sink.sampleCount == 1 && sink.arrayLength == 1 && sink.mipmapLevelCount == 1 && sink.textureType == .type2D &&
                 sink.storageMode == .private && sink.usage == .renderTarget && sink.hazardTrackingMode == .tracked &&
                 sink.buffer == nil && !sink.isFramebufferOnly, "actual texture getters")
        let trace = try buffer(p.guardBytes * 2 + p.count * p.trace_stride)
        let a = d.colorAttachments[0]!
        try need(a.pixelFormat == sink.pixelFormat && a.writeMask == .all && !a.isBlendingEnabled && d.rasterSampleCount == 1 &&
                 d.isRasterizationEnabled && !d.isAlphaToCoverageEnabled && !d.isAlphaToOneEnabled &&
                 d.depthAttachmentPixelFormat == .invalid && d.stencilAttachmentPixelFormat == .invalid, "pipeline getters")
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = sink
        pass.colorAttachments[0].loadAction = .dontCare; pass.colorAttachments[0].storeAction = .dontCare
        guard let cb = queue.makeCommandBuffer(), let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(message: "command or encoder unavailable") }
        e.setRenderPipelineState(pipeline); e.setCullMode(.none)
        e.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(p.width), height: Double(p.height), znear: 0, zfar: 1))
        e.setScissorRect(MTLScissorRect(x: 0, y: 0, width: p.width, height: p.height))
        e.setFragmentBuffer(inputs, offset: p.guardBytes, index: 0)
        e.setFragmentBuffer(trace, offset: p.guardBytes, index: 1)
        var params = SIMD4<UInt32>(UInt32(p.count), tag, skip, UInt32(p.width))
        e.setFragmentBytes(&params, length: 16, index: 2)
        e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3); e.endEncoding()
        try need(commands.count < 3, "command budget")
        cb.label = name; cb.commit(); cb.waitUntilCompleted()
        commands.append(["capture": name, "profile": profile, "status": cb.status.rawValue,
                         "error_present": cb.error != nil, "gpu_start": cb.gpuStartTime, "gpu_end": cb.gpuEndTime])
        let getters: [UInt] = [sink.pixelFormat.rawValue, UInt(sink.width), UInt(sink.height), UInt(d.rasterSampleCount),
            a.writeMask.rawValue, a.isBlendingEnabled ? 1 : 0, d.isRasterizationEnabled ? 1 : 0,
            d.isAlphaToCoverageEnabled ? 1 : 0, d.isAlphaToOneEnabled ? 1 : 0,
            pass.colorAttachments[0].loadAction.rawValue, pass.colorAttachments[0].storeAction.rawValue,
            1, 3, 1, UInt(p.count), UInt(inputs.length), UInt(trace.length)]
        receipts += ([String(tag), name, profile, String(cb.status.rawValue), cb.error == nil ? "0" : "1",
            String(cb.gpuStartTime), String(cb.gpuEndTime)] + getters.map(String.init)).joined(separator: "\t") + "\n"
        try need(cb.status == .completed && cb.error == nil, "GPU command failed")
        try store.save(name + ".bin", snapshot(trace)) // no conversion, endpoint repair, or filtering
        try need(snapshot(inputs) == before, "input data or guards changed")
        observations.append(["capture": name, "tag": tag, "profile": profile, "skip": skip,
            "return_precision": "scalar half operations; raw ushort observations", "pipeline_descriptor_getters": getters,
            "getter_scope": "descriptor used to create bound PSO, not opaque PSO introspection",
            "texture_storage": sink.storageMode.rawValue, "texture_usage": sink.usage.rawValue,
            "input_unchanged": true, "readback_after_completed": true])
        report["observations"] = observations
    }
    try need(commands.count == 3 && observations.count == 3, "capture count")
    try store.save("inputs-after.bin", snapshot(inputs))
    try store.save("complete.txt", Data("schema=1\nstatus=0\ncommands=3\ncaptures=3\ncases=24848\n".utf8))
    report["status"] = "CAPTURES_COMPLETE_PENDING_RUST_VALIDATION"
}
guard CommandLine.arguments.count == 3 else { exit(2) }
let store = Store(URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true))
var code: Int32 = 0
do { try run(URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true), store) }
catch { report["failure"] = errorInfo(error); code = 3 }
report["commands"] = commands; report["exit_code"] = code
do {
    try store.save("compiler.tsv", Data(compilerReceipts.utf8))
    try store.save("receipts.tsv", Data(receipts.utf8))
    report["raw_output_sha256"] = store.hashes; report["raw_output_bytes"] = store.bytes
    try store.save("gpu.json", JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]))
} catch { code = 4 }
print("native_capture_exit=\(code)")
exit(code)
