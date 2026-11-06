// New synthetic public-API probe; no external data. Integer sampling offsets
// are literal arguments in the MSL libraries. No shader ALU arithmetic test.
import Foundation
import Metal
import CryptoKit

struct Failure: Error { let stage: String; let message: String }
func need(_ c: Bool, _ stage: String, _ message: String) throws {
    if !c { throw Failure(stage: stage, message: message) }
}
func bytes<T>(_ a: [T]) -> Data { a.withUnsafeBytes { Data($0) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func hex(_ v: UInt32) -> String { String(format: "0x%08x", v) }
func words(_ d: Data) -> [UInt32] { d.withUnsafeBytes { Array($0.bindMemory(to: UInt32.self)) } }
func cls(_ w: UInt32) -> String {
    let m = w & 0x7fffffff
    if m == 0 { return "zero" }
    if m < 0x800000 { return "subnormal" }
    if m == 0x7f7fffff { return "flt_max" }
    return "normal"
}

var rngState: UInt64 = 0x9E3779B97F4A7C15
func rnd() -> UInt64 { rngState ^= rngState << 13; rngState ^= rngState >> 7; rngState ^= rngState << 17; return rngState }

let W = 16, H = 16, N = W * H * 4
// Arbitrary finite bits: signed zero, subnormals, FLT_MAX, and deterministic random words.
let specials: [UInt32] = [
    0x00000000, 0x80000000, 0x00000001, 0x80000001, 0x00000002, 0x80000002, 0x007fffff, 0x807fffff,
    0x00400000, 0x80400000, 0x00012345, 0x80012345, 0x00800000, 0x80800000, 0x00800001, 0x80800001,
    0x7f7fffff, 0xff7fffff, 0x7f7ffffe, 0xff7ffffe, 0x7f000000, 0xff000000, 0x3f800000, 0xbf800000,
    0x3f800001, 0x3f7fffff, 0x40490fdb, 0xc0490fdb, 0x33800000, 0x33000000, 0x33000001, 0x38800000,
    0x387fe000, 0x387fffff, 0x477fe000, 0x477fefff, 0x477ff000, 0x477ff001, 0xc77ff000, 0x3f801000,
    0x3f802000, 0x3f803000, 0x3f801001, 0x3dcccccd, 0xbdcccccd, 0x0000ffff, 0x00ff0000, 0x7effffff]
var dataA = specials
while dataA.count < N {
    let r = UInt32(truncatingIfNeeded: rnd() >> 11)
    let v = (dataA.count % 4 == 3) ? (r & 0x807fffff) : r
    if (v >> 23) & 0xff == 0xff { continue }
    dataA.append(v)
}


// Each offset visits all texels and the relevant shifted and original borders.
var coords: [SIMD2<Float>] = []
var meta: [SIMD2<UInt32>] = [] // offset id, cohort id
for dy in -8...7 { for dx in -8...7 {
    let id = UInt32((dy + 8) * 16 + dx + 8)
    func add(_ u: Float, _ v: Float, _ cohort: UInt32) {
        coords.append(SIMD2(u, v)); meta.append(SIMD2(id, cohort))
    }
    for y in 0..<H { for x in 0..<W {
        add(Float(x - dx) + 0.5, Float(y - dy) + 0.5, 0)
    } }
    // Every shifted texel edge, ±one binary32 ULP, on each axis.
    for k in 0...16 {
        let ex = Float(k - dx), ey = Float(k - dy)
        for e in [ex.nextDown, ex, ex.nextUp] { add(e, Float((k * 7) % H - dy) + 0.5, 1) }
        for e in [ey.nextDown, ey, ey.nextUp] { add(Float((k * 11) % W - dx) + 0.5, e, 1) }
    }
    var xs: [Float] = [], ys: [Float] = []
    for k in [0, 16] {
        let x = Float(k - dx), y = Float(k - dy)
        xs += [x.nextDown, x, x.nextUp]; ys += [y.nextDown, y, y.nextUp]
    }
    for v in ys { for u in xs { add(u, v, 2) } }
    let original: [Float] = [-1e6, -8.5, -1, -0.5, -Float.leastNonzeroMagnitude,
                              -0.0, 0.0, Float.leastNonzeroMagnitude, 0.5,
                              Float(16).nextDown, 16, Float(16).nextUp, 16.5, 23.5, 1e6]
    for v in original { for u in original { add(u, v, 3) } }
} }
let NC = coords.count
func indices(_ i: Int, offset: Bool) -> (Int, Int) {
    let id = Int(meta[i].x), dx = offset ? id % 16 - 8 : 0, dy = offset ? id / 16 - 8 : 0
    return (Int(Double(coords[i].x).rounded(.down)) + dx, Int(Double(coords[i].y).rounded(.down)) + dy)
}
func inside(_ x: Int, _ y: Int) -> Bool { x >= 0 && x < W && y >= 0 && y < H }
func expected(offset: Bool) -> [UInt32] {
    var e = [UInt32](); e.reserveCapacity(NC * 4)
    for i in 0..<NC {
        let (x, y) = indices(i, offset: offset)
        if inside(x, y) { e += dataA[((y * W + x) * 4)..<((y * W + x) * 4 + 4)] }
        else { e += [0, 0, 0, 0] }
    }
    return e
}
let expectedOffset = expected(offset: true), expectedPlain = expected(offset: false)
let outDir = URL(fileURLWithPath: CommandLine.arguments.count > 2 ? CommandLine.arguments[2] : ".")
var result: [String: Any] = ["schema": 1]
var pins: [String: String] = [:]
func save(_ name: String, _ d: Data) throws {
    try d.write(to: outDir.appendingPathComponent(name)); pins[name] = sha(d)
}
func analyze(_ g: [UInt32], _ e: [UInt32], offset: Bool) -> [String: Any] {
    var mis = 0, misSamples = 0, border = 0, rescued = 0, rescueMis = 0, borderMis = 0
    var byOffset = [Int](repeating: 0, count: 256), byCohort = [Int](repeating: 0, count: 4)
    var byClass: [String: Int] = [:], examples: [[String: Any]] = []
    for i in 0..<NC {
        let (x, y) = indices(i, offset: offset), (ux, uy) = indices(i, offset: false)
        let isBorder = !inside(x, y), isRescue = inside(x, y) && !inside(ux, uy)
        if isBorder { border += 1 }; if isRescue { rescued += 1 }
        var different = false
        for c in 0..<4 where g[i * 4 + c] != e[i * 4 + c] {
            different = true; mis += 1
            byClass[isBorder ? "border_positive_zero" : cls(e[i * 4 + c]), default: 0] += 1
            if examples.count < 48 {
                let id = Int(meta[i].x)
                examples.append(["query": i, "lane": c, "offset": [id % 16 - 8, id / 16 - 8],
                                 "uv_bits": [hex(coords[i].x.bitPattern), hex(coords[i].y.bitPattern)],
                                 "uv": [Double(coords[i].x), Double(coords[i].y)], "index": [x, y],
                                 "cohort": meta[i].y, "expected": hex(e[i*4+c]), "got": hex(g[i*4+c])])
            }
        }
        if different {
            misSamples += 1; byOffset[Int(meta[i].x)] += 1; byCohort[Int(meta[i].y)] += 1
            if isBorder { borderMis += 1 }; if isRescue { rescueMis += 1 }
        }
    }
    return ["mismatch_lanes": mis, "mismatch_samples": misSamples, "mismatch_samples_by_offset": byOffset,
            "mismatch_samples_by_cohort": byCohort, "mismatch_lanes_by_expected_class": byClass,
            "border_samples": border, "border_non_positive_zero_samples": borderMis,
            "outside_moved_inside_samples": rescued, "outside_moved_inside_mismatch_samples": rescueMis,
            "examples": examples]
}

func run() throws -> Int32 {
    try need(CommandLine.arguments.count == 3, "arguments", "usage: probe srcdir outdir")
    try need(NC == 158464 && meta.count == NC, "cohort", "unexpected query count")
    try save("input-u32.bin", bytes(dataA))
    try save("coords-f32x2.bin", bytes(coords))
    try save("meta-u32x2.bin", bytes(meta))
    try save("expected-offset-u32x4.bin", bytes(expectedOffset))
    result["layout"] = ["texture": [W, H], "pixel_format": 125, "bytes_per_pixel": 16,
                        "queries": NC, "offset_pairs": 256, "queries_per_offset": 619,
                        "offset_id": "(dy+8)*16+(dx+8)", "offset_domain": "[-8,7] on both axes",
                        "meta": "uint2(offset_id,cohort_id)", "cohorts": ["all_texel_centers", "shifted_edge_ulp_strips", "shifted_corner_ulp_grid", "original_coordinate_grid"],
                        "output": "little-endian uint4 per query, order identical to coords/meta",
                        "model": "integer floor(Double(binary32 coord)) + integer offset; out-of-range -> 4 positive zeros"] as [String: Any]
    result["input_classes"] = Dictionary(grouping: dataA, by: cls).mapValues { $0.count }
    guard let device = MTLCreateSystemDefaultDevice() else { throw Failure(stage: "device_allocation", message: "nil device") }
    var dev: [String: Any] = ["name": device.name, "registry_id": device.registryID, "unified_memory": device.hasUnifiedMemory]
    if #available(macOS 14.0, *) { dev["architecture"] = device.architecture.name }
    result["device"] = dev; result["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    guard let queue = device.makeCommandQueue() else { throw Failure(stage: "queue_allocation", message: "nil queue") }
    let sd = MTLSamplerDescriptor()
    sd.minFilter = .nearest; sd.magFilter = .nearest; sd.mipFilter = .notMipmapped
    sd.normalizedCoordinates = false; sd.maxAnisotropy = 1; sd.compareFunction = .never
    sd.sAddressMode = .clampToZero; sd.tAddressMode = .clampToZero; sd.rAddressMode = .clampToZero
    result["sampler"] = ["min": sd.minFilter.rawValue, "mag": sd.magFilter.rawValue, "mip": sd.mipFilter.rawValue,
                         "normalized": sd.normalizedCoordinates, "s": sd.sAddressMode.rawValue, "t": sd.tAddressMode.rawValue,
                         "r": sd.rAddressMode.rawValue, "max_anisotropy": sd.maxAnisotropy, "compare": sd.compareFunction.rawValue,
                         "lod_min": sd.lodMinClamp, "lod_max_is_flt_max": sd.lodMaxClamp == Float.greatestFiniteMagnitude] as [String: Any]
    guard let sampler = device.makeSamplerState(descriptor: sd) else { throw Failure(stage: "sampler_allocation", message: "nil sampler") }
    result["sampler_allocated"] = true
    let options = MTLCompileOptions()
    if #available(macOS 15.0, *) { result["math_mode"] = options.mathMode.rawValue }
    let src = URL(fileURLWithPath: CommandLine.arguments[1])
    var psos: [String: MTLComputePipelineState] = [:]
    var api: [String: Any] = [:]
    for file in ["base", "offset_nolod", "offset_level0"] {
        var info: [String: Any] = [:]
        do {
            let source = try String(contentsOf: src.appendingPathComponent(file + ".metal"), encoding: .utf8)
            let lib = try device.makeLibrary(source: source, options: options)
            info["library"] = "ok"
            for fn in (file == "base" ? ["store_f4", "read_f4", "sample_plain"] : ["observe_offset"]) {
                let key = file == "base" ? fn : file
                do {
                    guard let f = lib.makeFunction(name: fn) else { throw Failure(stage: "function", message: "missing function") }
                    psos[key] = try device.makeComputePipelineState(function: f); info[fn] = "ok"
                } catch { info[fn] = "PIPELINE_FAILURE: \(error)" }
            }
        } catch { info["library"] = "LIBRARY_FAILURE: \(error)" }
        api[file] = info
    }
    result["api"] = api
    func buffer(_ d: Data) throws -> MTLBuffer {
        let b = d.withUnsafeBytes { device.makeBuffer(bytes: $0.baseAddress!, length: $0.count, options: .storageModeShared) }
        guard let b else { throw Failure(stage: "buffer_allocation", message: "\(d.count) bytes") }; return b
    }
    func dispatch(_ name: String, _ tex: MTLTexture, _ input: MTLBuffer, _ count: Int, _ is2D: Bool,
                  _ metadata: MTLBuffer?) throws -> Data {
        guard let pso = psos[name] else { throw Failure(stage: "pipeline_unavailable", message: name) }
        let size = is2D ? N * 4 : count * 16
        let out = try buffer(Data(repeating: 0xa5, count: size))
        guard let cb = queue.makeCommandBuffer(), let enc = cb.makeComputeCommandEncoder() else {
            throw Failure(stage: "encoder_allocation", message: name)
        }
        enc.setComputePipelineState(pso); enc.setTexture(tex, index: 0)
        enc.setBuffer(input, offset: 0, index: 0); enc.setBuffer(out, offset: 0, index: 1)
        enc.setSamplerState(sampler, index: 0)
        if let metadata { enc.setBuffer(metadata, offset: 0, index: 3) }
        var dims = SIMD2<UInt32>(UInt32(is2D ? W : count), UInt32(is2D ? H : 1))
        enc.setBytes(&dims, length: 8, index: 2)
        let gw = is2D ? W : count, gh = is2D ? H : 1, th = is2D ? 8 : 1
        enc.dispatchThreadgroups(MTLSize(width: (gw+7)/8, height: (gh+th-1)/th, depth: 1),
                                 threadsPerThreadgroup: MTLSize(width: 8, height: th, depth: 1))
        enc.endEncoding(); cb.commit(); cb.waitUntilCompleted()
        try need(cb.status == .completed && cb.error == nil, "command_execution", "\(name) status \(cb.status.rawValue) error \(String(describing: cb.error))")
        return Data(bytes: out.contents(), count: size)
    }
    let input = try buffer(bytes(dataA)), uv = try buffer(bytes(coords)), metadata = try buffer(bytes(meta))
    var sections: [String: Any] = [:], anyFailure = false, anyMismatch = false
    let modes: [(String, MTLStorageMode)] = [("shared", .shared), ("private", .private)]
    for (tag, mode) in modes {
        var texture: MTLTexture? = nil
        do {
            let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba32Float, width: W, height: H, mipmapped: false)
            td.storageMode = mode; td.usage = [.shaderRead, .shaderWrite]
            guard let t = device.makeTexture(descriptor: td) else { throw Failure(stage: "texture_allocation", message: tag) }
            _ = try dispatch("store_f4", t, input, W*H, true, nil)
            texture = t
            let d = try dispatch("read_f4", t, input, W*H, true, nil)
            let name = "out-\(tag)-store-read.bin"
            try save(name, d)
            let count = zip(words(d), dataA).filter { $0 != $1 }.count
            sections[tag + "/store_read"] = ["status": "ok", "mismatch_lanes": count, "file": name] as [String: Any]
            anyMismatch = anyMismatch || count != 0
        } catch let e as Failure {
            sections[tag + "/store_read"] = ["status": "failure", "stage": e.stage, "error": e.message]; anyFailure = true
        } catch { sections[tag + "/store_read"] = ["status": "failure", "error": "\(error)"]; anyFailure = true }
        for key in ["sample_plain", "offset_nolod", "offset_level0"] {
            do {
                guard let t = texture else { throw Failure(stage: "dependency", message: "texture store unavailable") }
                let d = try dispatch(key, t, uv, NC, false, metadata)
                let name = "out-\(tag)-\(key).bin"
                try save(name, d)
                let offset = key != "sample_plain"
                var r = analyze(words(d), offset ? expectedOffset : expectedPlain, offset: offset)
                r["status"] = "ok"; r["file"] = name
                anyMismatch = anyMismatch || (r["mismatch_lanes"] as? Int) != 0
                sections[tag + "/" + key] = r
            } catch let e as Failure {
                sections[tag + "/" + key] = ["status": "failure", "stage": e.stage, "error": e.message]; anyFailure = true
            } catch { sections[tag + "/" + key] = ["status": "failure", "error": "\(error)"]; anyFailure = true }
        }
    }
    result["sections"] = sections
    result["status"] = anyFailure ? "NATIVE_API_OR_EXECUTION_FAILURE" : (anyMismatch ? "NUMERICAL_DIFFERENCE_OBSERVED" : "ALL_OFFSETS_ADMITTED_AND_EXACT")
    return anyFailure ? 3 : (anyMismatch ? 4 : 0)
}

var code: Int32 = 0
do { code = try run() }
catch let e as Failure { result["status"] = "NATIVE_SETUP_FAILURE"; result["failure_stage"] = e.stage; result["error"] = e.message; code = 3 }
catch { result["status"] = "PROBE_FAILURE"; result["error"] = "\(error)"; code = 3 }
result["files_sha256"] = pins
let json = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys])
try json.write(to: outDir.appendingPathComponent("summary.json"))
FileHandle.standardOutput.write(json)
exit(code)
