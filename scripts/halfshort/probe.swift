// Public Metal compute bridge. Loads the exact retained offline metallib bytes.
// No source-library fallback, CPU conversion fallback, or result normalization.
import Foundation
import Metal
import CryptoKit
import Dispatch

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
    return ["reason": "native_api_error", "domain": e.domain, "code": e.code]
}
struct Manifest: Decodable {
    let schema: Int
    let halfCount: Int
    let intCount: Int
    let guardBytes: Int
    let stride: Int
    let halfSkip: UInt32
    let intSkip: UInt32
}
final class Store {
    let out: URL
    var hashes: [String: String] = [:]
    var bytes = 0
    init(_ out: URL) { self.out = out }
    func save(_ name: String, _ data: Data) throws {
        try need(!name.contains("/") && !name.contains("\\") && !name.hasPrefix("."), "output name")
        try need(hashes[name] == nil && hashes.count < 24 && data.count <= 1_048_576 &&
                 bytes + data.count <= 7 * 1024 * 1024, "native artifact bounds")
        try data.write(to: out.appendingPathComponent(name), options: .withoutOverwriting)
        hashes[name] = sha(data); bytes += data.count
    }
}
var report: [String: Any] = ["schema": 1, "status": "UNQUALIFIED_NOT_COMPLETE", "cpu_fallback": false,
    "dynamic_source_library_fallback": false, "render_target_used": false]
var commands: [[String: Any]] = []
var captures: [[String: Any]] = []
var receipts = "capture\tmode\tnegative\tstatus\terror\tgpu_start\tgpu_end\tlibrary_sha256\n"

func run(_ sourceRoot: URL, _ store: Store) throws {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little-endian host required")
    var sources: [String: String] = [:]
    for name in ["cases.rs", "probe.metal", "probe.swift", "run.sh"] {
        sources[name] = sha(try Data(contentsOf: sourceRoot.appendingPathComponent(name)))
    }
    report["source_sha256"] = sources
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["toolchain_receipt"] = "../run-receipt.txt"
    report["semantics"] = "NaN/out-of-range results and fast-math inputs outside assumptions are diagnostic observations, not blanket defined MSL behavior"
    report["layout"] = ["guard_bytes": 64, "stride": 12,
        "half-short": "ushort result @0, untouched ushort @2, ushort actual input echo @4, untouched ushort @6, uint marker @8",
        "half-int": "uint result bits @0, ushort actual input echo @4, untouched ushort @6, uint marker @8",
        "int-half": "ushort half result bits @0, untouched ushort @2, uint actual input bits @4, uint marker @8",
        "marker": "0x5a000000 | (unique dispatch tag << 16) | row; data files include prefix/suffix guards",
        "initial_fill": "every result byte starts at 0xcd; a data value equal to poison is not rejected; independent markers establish coverage"]
    func input(_ name: String) throws -> Data {
        let d = try Data(contentsOf: store.out.appendingPathComponent(name))
        try need(d.count <= 1_048_576, "input cap")
        return d
    }
    let manifestData = try input("cases.json"), halfData = try input("half-input.bin"), intData = try input("int-input.bin")
    let m = try JSONDecoder().decode(Manifest.self, from: manifestData)
    try need(m.schema == 1 && m.halfCount == 65536 && m.intCount == 31 && m.guardBytes == 64 &&
             m.stride == 12 && m.halfSkip == 0x3c00 && m.intSkip == 1, "manifest bounds")
    try need(halfData.count == 131072 && intData.count == 124, "input lengths")
    report["input_sha256"] = ["cases.json": sha(manifestData), "half-input.bin": sha(halfData), "int-input.bin": sha(intData)]
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(message: "Metal device or command queue unavailable; no fallback")
    }
    var families: [String: Bool] = [:]
    let queries: [(String, MTLGPUFamily)] = [("apple1", .apple1), ("apple2", .apple2), ("apple3", .apple3),
        ("apple4", .apple4), ("apple5", .apple5), ("apple6", .apple6), ("apple7", .apple7), ("mac2", .mac2)]
    for (name, family) in queries { families[name] = dev.supportsFamily(family) }
    if #available(macOS 13.0, *) {
        families["apple8"] = dev.supportsFamily(.apple8); families["metal3"] = dev.supportsFamily(.metal3)
    }
    if #available(macOS 14.0, *) { families["apple9"] = dev.supportsFamily(.apple9) }
    var device: [String: Any] = ["name": dev.name, "registry_id": String(dev.registryID),
        "has_unified_memory": dev.hasUnifiedMemory, "is_low_power": dev.isLowPower, "is_removable": dev.isRemovable,
        "max_buffer_length": dev.maxBufferLength, "family_queries": families, "family_queries_exhaustive": false]
    if #available(macOS 14.0, *) { device["architecture"] = dev.architecture.name }
    report["device"] = device
    try need(MTLCommandBufferStatus.completed.rawValue == 4, "native command status enum")

    func buffer(_ length: Int) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: length, options: .storageModeShared) else { throw Failure(message: "shared buffer allocation") }
        try need(b.storageMode == .shared && b.length == length, "actual buffer mismatch")
        memset(b.contents(), 0xcd, b.length)
        return b
    }
    func upload(_ data: Data) throws -> MTLBuffer {
        let b = try buffer(data.count + 128)
        data.withUnsafeBytes { b.contents().advanced(by: 64).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
        return b
    }
    func raw(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let halfInput = try upload(halfData), intInput = try upload(intData)
    let halfBefore = raw(halfInput), intBefore = raw(intInput)
    let entries = ["convert_half_short", "convert_half_int", "convert_int_half"]
    let names = ["half-short", "half-int", "int-half"]
    let counts = [m.halfCount, m.halfCount, m.intCount]
    var libraries: [[String: Any]] = []
    for (modeIndex, mode) in ["nofast", "fast"].enumerated() {
        var artifactHashes: [String: String] = [:]
        var libraryBytes = Data()
        for ext in ["air", "ll", "metallib"] {
            let name = mode + "." + ext
            let d = try input(name)
            try need(!d.isEmpty, "empty offline artifact")
            artifactHashes[name] = sha(d)
            if ext == "metallib" { libraryBytes = d }
        }
        let proofData = try input(mode + "-air-proof.json")
        let buildData = try input(mode + "-compile.json")
        artifactHashes[mode + "-air-proof.json"] = sha(proofData)
        artifactHashes[mode + "-compile.json"] = sha(buildData)
        let proof = try JSONSerialization.jsonObject(with: proofData) as? [String: Any]
        let build = try JSONSerialization.jsonObject(with: buildData) as? [String: Any]
        try need(build?["compile_exit"] as? Int == 0 && build?["inspect_exit"] as? Int == 0 &&
                 build?["link_exit"] as? Int == 0 && build?["fast_math_requested"] as? Bool == (modeIndex == 1), "offline compiler chain incomplete")
        try need(build?["air_sha256"] as? String == artifactHashes[mode + ".air"] &&
                 build?["ir_sha256"] as? String == artifactHashes[mode + ".ll"] &&
                 build?["proof_sha256"] as? String == sha(proofData) &&
                 build?["library_sha256"] as? String == sha(libraryBytes) &&
                 build?["link_input"] as? String == mode + ".air" &&
                 build?["runtime_library"] as? String == mode + ".metallib", "offline artifact binding changed")
        try need(Array(libraryBytes.prefix(4)) == [0x4d, 0x54, 0x4c, 0x42], "metallib signature")
        // Bind the loaded library to these exact SHA256-recorded bytes, not to
        // a separately source-compiled sibling or a path reloaded after hashing.
        let dispatchData = libraryBytes.withUnsafeBytes { DispatchData(bytes: $0) }
        let library = try dev.makeLibrary(data: dispatchData)
        try need(Set(library.functionNames) == Set(entries), "unexpected offline entry points")
        let libraryHash = sha(libraryBytes)
        var pipelines: [MTLComputePipelineState] = []
        var pipelineInfo: [[String: Any]] = []
        for entry in entries {
            guard let function = library.makeFunction(name: entry) else { throw Failure(message: "missing compute entry") }
            try need(function.functionType == .kernel, "entry is not a compute kernel")
            let pso = try dev.makeComputePipelineState(function: function)
            pipelines.append(pso)
            pipelineInfo.append(["entry": entry, "max_threads": pso.maxTotalThreadsPerThreadgroup,
                                 "execution_width": pso.threadExecutionWidth, "function_type": function.functionType.rawValue])
        }
        libraries.append(["mode": mode, "fast_math_requested": modeIndex == 1,
            "load_route": "makeLibrary(data:) from exact retained metallib bytes", "loaded_sha256": libraryHash,
            "offline_artifact_sha256": artifactHashes, "air_signature_qualified": proof?["qualified"] as? Bool ?? false,
            "compile_receipt": build ?? [:], "pipelines": pipelineInfo])
        report["libraries"] = libraries
        for negative in [false, true] {
            let capture = modeIndex * 2 + (negative ? 1 : 0)
            let label = mode + (negative ? "-negative" : "-positive")
            let outputs = try counts.map { try buffer($0 * 12 + 128) }
            guard let cb = queue.makeCommandBuffer() else { throw Failure(message: "command allocation") }
            cb.label = label
            var dispatches: [[String: Any]] = []
            for k in 0..<3 {
                guard let encoder = cb.makeComputeCommandEncoder() else { throw Failure(message: "compute encoder allocation") }
                let inputBuffer = k == 2 ? intInput : halfInput
                let skip = negative ? (k == 2 ? m.intSkip : m.halfSkip) : UInt32.max
                let tag = UInt32(capture * 3 + k + 1)
                var params = SIMD4<UInt32>(UInt32(counts[k]), tag, skip, 0)
                encoder.setComputePipelineState(pipelines[k])
                encoder.setBuffer(inputBuffer, offset: 64, index: 0)
                encoder.setBuffer(outputs[k], offset: 64, index: 1)
                encoder.setBytes(&params, length: 16, index: 2)
                let threads = min(64, pipelines[k].maxTotalThreadsPerThreadgroup)
                try need(threads > 0, "invalid threadgroup limit")
                encoder.dispatchThreadgroups(MTLSize(width: (counts[k] + threads - 1) / threads, height: 1, depth: 1),
                    threadsPerThreadgroup: MTLSize(width: threads, height: 1, depth: 1))
                encoder.endEncoding()
                dispatches.append(["kernel": entries[k], "rows": counts[k], "tag": tag, "skip": skip,
                    "output_length": outputs[k].length, "actual_storage": outputs[k].storageMode.rawValue,
                    "threads_per_group": threads, "groups": (counts[k] + threads - 1) / threads])
            }
            cb.commit(); cb.waitUntilCompleted()
            var command: [String: Any] = ["capture": label, "mode": mode, "negative": negative,
                "status": cb.status.rawValue, "completed": cb.status == .completed, "error_present": cb.error != nil,
                "gpu_start": cb.gpuStartTime, "gpu_end": cb.gpuEndTime, "library_sha256": libraryHash,
                "timing_note": "zero timestamps are unavailable, not invented"]
            if let error = cb.error { command["error"] = errorInfo(error) }
            commands.append(command)
            receipts += [label, mode, negative ? "1" : "0", String(cb.status.rawValue), cb.error == nil ? "0" : "1",
                         String(cb.gpuStartTime), String(cb.gpuEndTime), libraryHash].joined(separator: "\t") + "\n"
            try need(cb.status == .completed && cb.error == nil, "GPU command did not complete")
            for k in 0..<3 { try store.save(label + "-" + names[k] + ".bin", raw(outputs[k])) }
            try need(raw(halfInput) == halfBefore && raw(intInput) == intBefore, "input data/guards changed")
            captures.append(["name": label, "mode": mode, "negative": negative, "dispatches": dispatches,
                "input_buffers_unchanged": true, "status": "CAPTURED_PENDING_ORDINARY_VALIDATION"])
        }
    }
    try need(commands.count == 4 && captures.count == 4 && libraries.count == 2, "capture coverage")
    report["status"] = "OFFLINE_NATIVE_CAPTURES_COMPLETE_PENDING_AIR_AND_RAW_GATES"
}

guard CommandLine.arguments.count == 3 else { exit(2) }
let store = Store(URL(fileURLWithPath: CommandLine.arguments[2], isDirectory: true))
var code: Int32 = 0
do { try run(URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true), store) }
catch { report["failure"] = errorInfo(error); code = 3 }
report["commands"] = commands; report["captures"] = captures; report["exit_code"] = code
do {
    try store.save("receipts.tsv", Data(receipts.utf8))
    report["raw_output_sha256"] = store.hashes; report["raw_output_bytes"] = store.bytes
    try store.save("gpu.json", JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]))
} catch { code = 4 }
print("offline_native_capture_exit=\(code)")
exit(code)
