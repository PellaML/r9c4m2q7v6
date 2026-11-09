import Foundation
import Metal
import CryptoKit
import Dispatch

struct Failure: Error { let reason: String }
func need(_ ok: Bool, _ reason: String) throws { if !ok { throw Failure(reason: reason) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func info(_ error: Error) -> [String: Any] {
    if let f = error as? Failure { return ["reason": f.reason] }
    let e = error as NSError; return ["reason": "native_api_error", "domain": e.domain, "code": e.code]
}
func u32(_ d: Data, _ i: Int) -> UInt32 {
    UInt32(d[i]) | UInt32(d[i+1]) << 8 | UInt32(d[i+2]) << 16 | UInt32(d[i+3]) << 24
}
func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
struct Plan: Decodable { let schema: Int; let count: Int; let inverseCount: Int; let nearestCount: Int; let guardBytes: Int }
guard CommandLine.arguments.count == 4 else { exit(2) }
let mode = CommandLine.arguments[1]
guard ["precise", "fast"].contains(mode) else { exit(2) }
let source = URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true)
let out = URL(fileURLWithPath: CommandLine.arguments[3], isDirectory: true)
for suffix in [".gpu.json", ".gpu.bin"] {
    guard !FileManager.default.fileExists(atPath: out.appendingPathComponent(mode + suffix).path) else { exit(2) }
}
var report: [String: Any] = ["schema": 1, "mode": mode, "status": "FAILED_NOT_PASS",
    "native_started": false, "cpu_result_fallback": false, "source_compile_fallback": false,
    "numeric_policy": "raw observations only; no CPU-generated expected conversion outputs"]
var commands: [[String: Any]] = [], records: [[String: Any]] = []
var raw = Data()
func read(_ name: String) throws -> Data {
    let d = try Data(contentsOf: out.appendingPathComponent(name))
    try need(d.count <= 1_048_576, "input_budget"); return d
}
func checkpoint(_ stage: String) throws {
    report["stage"] = stage; report["commands"] = commands; report["records"] = records
    report["raw_sha256"] = sha(raw); report["raw_bytes"] = raw.count
    let d = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
    try need(d.count < 131072, "report_budget")
    try d.write(to: out.appendingPathComponent(mode + ".gpu.json"))
}
func save(_ d: Data, _ fields: [String: Any]) throws {
    try need(records.count < 16 && raw.count + d.count < 65536, "native_output_budget")
    var r = fields; r["offset"] = raw.count; r["length"] = d.count; r["sha256"] = sha(d)
    records.append(r); raw.append(d)
    try raw.write(to: out.appendingPathComponent(mode + ".gpu.bin"))
    try checkpoint("saved_" + (fields["name"] as? String ?? "record"))
}

func run() throws {
    let planData = try read("inputs.json"), floats = try read("float-input.bin"), standalone = try read("inverse-input.bin")
    let p = try JSONDecoder().decode(Plan.self, from: planData)
    try need(p.schema == 1 && p.count == 20 && p.inverseCount == 3 && p.nearestCount == 4 && p.guardBytes == 64 &&
             floats.count == p.count*8 && standalone.count == p.inverseCount*4, "fixed_input_layout")
    // Domain refusal happens before device creation, pipeline creation or dispatch.
    for i in 0..<(p.count*2) {
        let x = Float(bitPattern: u32(floats, i*4))
        try need(x.isFinite && x > -32769 && x < 32768, "CPU_REFUSED_outside_finite_open_RTZ_domain")
        if i < p.nearestCount*2 { try need(abs(x) <= 16, "CPU_REFUSED_unsafe_rounding_negative") }
    }
    report["domain"] = "all forward lanes finite and -32769 < x < 32768; rounding negative restricted to abs(x)<=16"
    report["input_sha256"] = ["inputs.json": sha(planData), "float-input.bin": sha(floats), "inverse-input.bin": sha(standalone)]
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little_endian_required")
    let air = try read(mode + ".air"), ir = try read(mode + ".ll"), proofData = try read(mode + ".ir-proof.json")
    let lib = try read(mode + ".metallib"), pairData = try read(mode + ".compile.json")
    let pair = try JSONSerialization.jsonObject(with: pairData) as? [String: Any]
    let proof = try JSONSerialization.jsonObject(with: proofData) as? [String: Any]
    let msl = try Data(contentsOf: source.appendingPathComponent("probe.metal"))
    try need(pair?["mode"] as? String == mode && pair?["source_sha256"] as? String == sha(msl) &&
        pair?["air_sha256"] as? String == sha(air) && pair?["ir_sha256"] as? String == sha(ir) &&
        pair?["proof_sha256"] as? String == sha(proofData) && pair?["library_sha256"] as? String == sha(lib) &&
        pair?["link_input"] as? String == mode + ".air" && pair?["compile_exit"] as? Int == 0 &&
        pair?["inspect_exit"] as? Int == 0 && pair?["link_exit"] as? Int == 0 &&
        pair?["sibling_source_ir_used"] as? Bool == false && proof?["qualified"] as? Bool == true &&
        proof?["ir_sha256"] as? String == sha(ir) && !air.isEmpty && lib.prefix(4) == Data([0x4d,0x54,0x4c,0x42]),
        "exact_AIR_IR_proof_library_chain_required")
    report["compile_pair"] = pair; report["loaded_library_sha256"] = sha(lib)
    try checkpoint("domain_and_offline_proof_checked")
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else { throw Failure(reason: "Metal_device_or_queue_nil_no_substitute") }
    report["device"] = ["name": dev.name, "registry_id": String(dev.registryID), "unified_memory": dev.hasUnifiedMemory,
        "low_power": dev.isLowPower, "removable": dev.isRemovable, "mac2": dev.supportsFamily(.mac2), "metal3": dev.supportsFamily(.metal3)]
    if #available(macOS 14.0, *) { report["device_architecture"] = dev.architecture.name }
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["os_build_from_runner"] = ProcessInfo.processInfo.environment["NATIVE_OS_BUILD"] ?? "missing"
    let library = try dev.makeLibrary(data: lib.withUnsafeBytes { DispatchData(bytes: $0) })
    let names = ["forward_s16", "inverse_s16", "nearest_s16", "swap_s16"]
    try need(Set(library.functionNames) == Set(names), "offline_entry_points")
    var pipelines: [String: MTLComputePipelineState] = [:]
    for name in names {
        guard let f = library.makeFunction(name: name) else { throw Failure(reason: "kernel_missing") }
        let pipeline = try dev.makeComputePipelineState(function: f)
        try need(pipeline.maxTotalThreadsPerThreadgroup >= 16, "threadgroup_unsupported")
        pipelines[name] = pipeline
    }
    func buffer(_ bodyBytes: Int) throws -> MTLBuffer {
        let length = bodyBytes + 128
        guard let b = dev.makeBuffer(length: length, options: [.storageModeShared, .hazardTrackingModeTracked]) else {
            throw Failure(reason: "shared_buffer_nil_no_substitute")
        }
        try need(b.length == length && b.storageMode == .shared && b.hazardTrackingMode == .tracked, "actual_buffer_properties")
        memset(b.contents(), 0xcd, length); return b
    }
    func upload(_ data: Data) throws -> MTLBuffer {
        let b = try buffer(data.count)
        data.withUnsafeBytes { b.contents().advanced(by: 64).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
        return b
    }
    let input = try upload(floats), independent = try upload(standalone)
    var tag: UInt32 = 0
    func dispatch(_ name: String, _ kernel: String, _ src: MTLBuffer, _ count: Int,
                  _ stride: Int, _ inputLabel: String) throws -> MTLBuffer {
        tag += 1
        let before = snapshot(src), result = try buffer(count*stride), trace = try buffer(count*16)
        guard let cb = queue.makeCommandBuffer(), let e = cb.makeComputeCommandEncoder(), let pipeline = pipelines[kernel] else {
            throw Failure(reason: "command_or_encoder_nil")
        }
        e.setComputePipelineState(pipeline)
        e.setBuffer(src, offset: 64, index: 0); e.setBuffer(result, offset: 64, index: 1)
        e.setBuffer(trace, offset: 64, index: 2)
        var params = SIMD4<UInt32>(UInt32(count), tag, 0, 0)
        e.setBytes(&params, length: 16, index: 3)
        e.dispatchThreads(MTLSize(width: count, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 16, height: 1, depth: 1))
        e.endEncoding(); report["native_started"] = true
        cb.label = name; cb.commit(); cb.waitUntilCompleted()
        var c: [String: Any] = ["name": name, "tag": tag, "kernel": kernel, "count": count, "input": inputLabel,
            "input_sha256_before": sha(before), "input_sha256_after": sha(snapshot(src)),
            "storage": result.storageMode.rawValue, "status": cb.status.rawValue, "completed": cb.status == .completed,
            "error_present": cb.error != nil, "gpu_start": cb.gpuStartTime, "gpu_end": cb.gpuEndTime]
        if let error = cb.error { c["error"] = info(error) }
        commands.append(c); try checkpoint("command_" + name)
        try need(cb.status == .completed && cb.error == nil && snapshot(src) == before, "command_or_input_mutation_failure")
        let fields: [String: Any] = ["name": name, "tag": tag, "count": count, "kernel": kernel, "input": inputLabel]
        var r = fields; r["kind"] = stride == 4 ? "i16" : "f32"; try save(snapshot(result), r)
        r["kind"] = "trace"; try save(snapshot(trace), r)
        return result
    }
    let produced = try dispatch("forward", "forward_s16", input, 20, 4, "cpu:float-input.bin")
    // Same actual GPU buffer: no CPU copy/re-upload/quantization between dispatches.
    _ = try dispatch("chain", "inverse_s16", produced, 20, 8, "gpu:forward")
    _ = try dispatch("standalone", "inverse_s16", independent, 3, 8, "cpu:inverse-input.bin")
    let swapped = try dispatch("swap", "swap_s16", input, 20, 4, "cpu:float-input.bin")
    _ = try dispatch("swap-chain", "inverse_s16", swapped, 20, 8, "gpu:swap")
    let rounded = try dispatch("nearest", "nearest_s16", input, 4, 4, "cpu:float-input.bin:first4-safe")
    _ = try dispatch("nearest-chain", "inverse_s16", rounded, 4, 8, "gpu:nearest")
    try need(commands.count == 7 && records.count == 14, "capture_coverage")
    report["same_GPU_intermediate_buffer_consumed"] = true
    report["intermediate_saved_before_inverse"] = true
    report["status"] = "CAPTURED_PENDING_RAW_CONTROL_VALIDATION"
}
var code: Int32 = 0
do { try run() }
catch {
    report["failure"] = info(error); code = 3
    if let f = error as? Failure, f.reason.hasPrefix("CPU_REFUSED") { report["status"] = "CPU_REFUSED_NO_NATIVE_DISPATCH" }
}
report["exit_code"] = code
do { try checkpoint("finished") } catch { code = 4 }
print("native_exit=\(code)")
exit(code)
