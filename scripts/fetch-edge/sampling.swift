// Minimal native API bridge. Capture bytes; do not predict sampling arithmetic.
import Foundation
import Metal
import CryptoKit
import Darwin
struct Failure: Error { let reason: String }
func need(_ ok: Bool, _ reason: String) throws { if !ok { throw Failure(reason: reason) } }
func sha(_ d: Data) -> String { SHA256.hash(data: d).map { String(format: "%02x", $0) }.joined() }
func details(_ error: Error) -> [String: Any] {
    if let e = error as? Failure { return ["reason": e.reason] }
    let e = error as NSError // No arbitrary descriptions, paths, URLs or userInfo.
    return ["domain": String(e.domain.prefix(128)), "code": e.code]
}
func osBuild() throws -> String {
    var n = 0
    try need(sysctlbyname("kern.osversion", nil, &n, nil, 0) == 0 && n > 1 && n < 128, "OS_build_size")
    var b = [CChar](repeating: 0, count: n)
    let rc = b.withUnsafeMutableBytes { sysctlbyname("kern.osversion", $0.baseAddress, &n, nil, 0) }
    try need(rc == 0, "OS_build_unavailable"); return String(cString: b)
}
let args = CommandLine.arguments
guard args.count == 3 else { exit(64) }
let root = URL(fileURLWithPath: args[2], isDirectory: true)
guard !["native.tsv", "native-baseline.json", "native-status.txt"].contains(where: {
    FileManager.default.fileExists(atPath: root.appendingPathComponent($0).path)
}) else { exit(64) }
var status = "FAILED"
var info: [String: Any] = ["schema": 1, "arithmetic_model": false, "scope": "actual native API capture only"]
let header = "id\ttexture\tkind\tmode\tpixel\trows\tskip\tformat\twidth\theight\tdepth\tmips\tarray\tsamples\tstorage\tusage\tnormalized\tmin\tmag\tmip\ts\tt\tr\tanisotropy\tlod_min_bits\tlod_max_bits\tcommitted\twait_returned\tcommand_status\terror_present\traw_bytes\n"
var receipt = header
func flush() throws {
    info["status"] = status
    let data = try JSONSerialization.data(withJSONObject: info, options: [.prettyPrinted, .sortedKeys])
    try need(data.count <= 65536 && receipt.utf8.count <= 65536, "metadata_budget")
    try data.write(to: root.appendingPathComponent("native-baseline.json"), options: .atomic)
    try receipt.write(to: root.appendingPathComponent("native.tsv"), atomically: true, encoding: .utf8)
    try (status + "\n").write(to: root.appendingPathComponent("native-status.txt"), atomically: true, encoding: .utf8)
}
func save(_ name: String, _ data: Data) throws {
    try need(data.count <= 131072, "raw_file_budget")
    try data.write(to: root.appendingPathComponent(name), options: .withoutOverwriting)
}
do {
    info["os_build"] = try osBuild(); info["os_version_and_build"] = ProcessInfo.processInfo.operatingSystemVersionString
    guard let framework = Bundle(path: "/System/Library/Frameworks/Metal.framework"),
          let build = framework.object(forInfoDictionaryKey: "CFBundleVersion") as? String, !build.isEmpty
    else { throw Failure(reason: "Metal_framework_build_unavailable") }
    info["Metal_framework"] = ["identifier": framework.bundleIdentifier ?? "unavailable", "build": build,
        "short_version": framework.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unavailable"]
    guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue()
    else { throw Failure(reason: "native_Metal_device_or_queue_unavailable") }
    info["device"] = ["name": device.name, "registry_id": String(device.registryID), "unified_memory": device.hasUnifiedMemory,
                      "low_power": device.isLowPower, "removable": device.isRemovable, "mac2": device.supportsFamily(.mac2)]
    try flush()
    let source = try Data(contentsOf: URL(fileURLWithPath: args[1]))
    guard let text = String(data: source, encoding: .utf8) else { throw Failure(reason: "MSL_UTF8") }
    let options = MTLCompileOptions(); options.fastMathEnabled = false
    info["MSL_sha256"] = sha(source); info["compiler"] = ["fast_math": options.fastMathEnabled, "language_version": options.languageVersion.rawValue]
    let lib = try device.makeLibrary(source: text, options: options)
    func pipeline(_ name: String) throws -> MTLComputePipelineState {
        guard let f = lib.makeFunction(name: name) else { throw Failure(reason: "kernel_missing") }
        let p = try device.makeComputePipelineState(function: f)
        try need(p.maxTotalThreadsPerThreadgroup >= 64, "threadgroup_capacity"); return p
    }
    let reader = try pipeline("read_capture"), sample = try pipeline("sample_capture")
    let coords = try Data(contentsOf: root.appendingPathComponent("coords.bin"))
    try need(coords.count == 1220 * 16, "coordinate_size")
    guard let uv = coords.withUnsafeBytes({ device.makeBuffer(bytes: $0.baseAddress!, length: $0.count, options: .storageModeShared) })
    else { throw Failure(reason: "coordinate_buffer") }
    let manifest = try String(contentsOf: root.appendingPathComponent("cases.tsv"), encoding: .utf8)
    let lines = manifest.split(separator: "\n").dropFirst()
    try need(lines.count == 13, "case_count")
    try need(MTLPixelFormat.rgb10a2Unorm.rawValue == 90, "format_enum")
    var textures: [MTLTexture] = [], staging: [MTLBuffer] = [], textureInfo: [[String: Any]] = []
    status = "ACQUIRING"; try flush()
    for (serial, line) in lines.enumerated() {
        let v = line.split(separator: "\t").compactMap { UInt32($0) }
        try need(v.count == 7 && v[0] == UInt32(serial) && v[1] < 2 && v[2] < 4 && [0, 1, 3, 5, 7, 8].contains(v[3]) && v[4] <= 1 &&
                 (v[5] == 35 || v[5] == 1220) && (v[6] == UInt32.max || (serial == 12 && v[6] == 0)), "case_framing")
        let id = Int(v[0]), ti = Int(v[1]), kind = Int(v[2]), mode = Int(v[3]), count = Int(v[5])
        info["active_case"] = id; try flush()
        if kind == 0 {
            try need(ti == textures.count && count == 35, "texture_order")
            let format = MTLPixelFormat.rgb10a2Unorm, bpp = 4
            let seed = try Data(contentsOf: root.appendingPathComponent("seed\(ti).bin"))
            try need(seed.count == 35 * bpp, "seed_size")
            let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: format, width: 7, height: 5, mipmapped: false)
            d.storageMode = .private; d.usage = .shaderRead
            d.swizzle = MTLTextureSwizzleChannels(red: .red, green: .green, blue: .blue, alpha: .alpha)
            guard let t = device.makeTexture(descriptor: d), let upload = device.makeBuffer(length: 1792, options: .storageModeShared)
            else { throw Failure(reason: "texture_or_upload_allocation_no_fallback") }
            let sw = t.swizzle
            try need(t.pixelFormat == format && t.width == 7 && t.height == 5 && t.depth == 1 && t.mipmapLevelCount == 1 &&
                     t.arrayLength == 1 && t.sampleCount == 1 && t.textureType == .type2D && t.storageMode == .private &&
                     t.usage == .shaderRead && t.buffer == nil && !t.isFramebufferOnly && sw.red == .red && sw.green == .green &&
                     sw.blue == .blue && sw.alpha == .alpha, "actual_texture_contract")
            memset(upload.contents(), 0xcd, upload.length)
            seed.withUnsafeBytes { b in for y in 0..<5 { memcpy(upload.contents().advanced(by: 256 + y * 256), b.baseAddress!.advanced(by: y * 7 * bpp), 7 * bpp) } }
            textures.append(t); staging.append(upload)
            textureInfo.append(["id": ti, "format": t.pixelFormat.rawValue, "width": t.width, "height": t.height,
                "storage": t.storageMode.rawValue, "usage": t.usage.rawValue, "mips": t.mipmapLevelCount,
                "seed_sha256": sha(seed), "identity_swizzle": true, "buffer_backed": t.buffer != nil])
            info["textures"] = textureInfo
        }
        try need(ti < textures.count, "texture_missing")
        let t = textures[ti], length = 512 + 64 * count
        guard let output = device.makeBuffer(length: length, options: .storageModeShared),
              let transfer = device.makeBuffer(length: 1792, options: .storageModeShared)
        else { throw Failure(reason: "readback_allocation") }
        memset(output.contents(), 0xcd, output.length); memset(transfer.contents(), 0xcd, transfer.length)
        let cd = MTLCommandBufferDescriptor(); cd.errorOptions = .encoderExecutionStatus
        guard let cb = queue.makeCommandBuffer(descriptor: cd) else { throw Failure(reason: "command_allocation") }
        cb.label = "owned-sampling-\(id)"
        if kind == 0 {
            guard let b = cb.makeBlitCommandEncoder() else { throw Failure(reason: "upload_encoder") }
            b.copy(from: staging[ti], sourceOffset: 256, sourceBytesPerRow: 256, sourceBytesPerImage: 1280,
                   sourceSize: MTLSize(width: 7, height: 5, depth: 1), to: t, destinationSlice: 0, destinationLevel: 0,
                   destinationOrigin: MTLOrigin(x: 0, y: 0, z: 0)); b.endEncoding()
        }
        guard let e = cb.makeComputeCommandEncoder() else { throw Failure(reason: "compute_encoder") }
        e.label = "owned-sample-capture"; e.setComputePipelineState(kind == 0 ? reader : sample)
        e.setTexture(t, index: 0); e.setTexture(t, index: 1); e.setBuffer(uv, offset: 0, index: 0)
        e.setBuffer(output, offset: 256, index: 1)
        let params = [v[5], v[0], v[3], v[4], v[6]]
        params.withUnsafeBytes { e.setBytes($0.baseAddress!, length: $0.count, index: 2) }
        var samplerFields = [String](repeating: "-", count: 10)
        if kind != 0 {
            let s = MTLSamplerDescriptor(); s.normalizedCoordinates = mode & 2 == 0
            s.minFilter = mode & 1 == 0 ? .nearest : .linear; s.magFilter = s.minFilter; s.mipFilter = .notMipmapped
            s.sAddressMode = mode & 4 == 0 ? .clampToEdge : .clampToZero; s.tAddressMode = s.sAddressMode; s.rAddressMode = s.sAddressMode
            s.maxAnisotropy = 1; s.lodMinClamp = 0; s.lodMaxClamp = 0; s.compareFunction = .never
            guard let state = device.makeSamplerState(descriptor: s) else { throw Failure(reason: "sampler_allocation") }
            e.setSamplerState(state, index: 0)
            samplerFields = [s.normalizedCoordinates ? "1" : "0", String(s.minFilter.rawValue), String(s.magFilter.rawValue),
                String(s.mipFilter.rawValue), String(s.sAddressMode.rawValue), String(s.tAddressMode.rawValue), String(s.rAddressMode.rawValue),
                String(s.maxAnisotropy), String(s.lodMinClamp.bitPattern), String(s.lodMaxClamp.bitPattern)]
        }
        e.dispatchThreadgroups(MTLSize(width: (count + 63) / 64, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 64, height: 1, depth: 1))
        e.endEncoding()
        if kind == 0 {
            guard let b = cb.makeBlitCommandEncoder() else { throw Failure(reason: "input_roundtrip_encoder") }
            b.copy(from: t, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                   sourceSize: MTLSize(width: 7, height: 5, depth: 1), to: transfer, destinationOffset: 256,
                   destinationBytesPerRow: 256, destinationBytesPerImage: 1280); b.endEncoding()
        }
        cb.commit(); cb.waitUntilCompleted()
        // Save complete buffers before checking command success or any values.
        try save(String(format: "case%02d.bin", id), Data(bytes: output.contents(), count: output.length))
        if kind == 0 { try save("transfer\(ti).bin", Data(bytes: staging[ti].contents(), count: 1792) + Data(bytes: transfer.contents(), count: 1792)) }
        let actual = [t.pixelFormat.rawValue, UInt(t.width), UInt(t.height), UInt(t.depth), UInt(t.mipmapLevelCount),
                      UInt(t.arrayLength), UInt(t.sampleCount), t.storageMode.rawValue, t.usage.rawValue].map { String($0) }
        receipt += String(line) + "\t" + (actual + samplerFields + ["1", "1", String(cb.status.rawValue), cb.error == nil ? "0" : "1", String(length)]).joined(separator: "\t") + "\n"
        if let error = cb.error { info["command_error"] = details(error) }
        try flush(); try need(cb.status == .completed && cb.error == nil, "command_not_cleanly_completed")
    }
    info["captured_cases"] = 13; status = "CAPTURED_ALL"; try flush()
} catch {
    status = "FAILED"; info["failure"] = details(error)
    do { try flush() } catch { fputs("native receipt write failed\n", stderr) }
    exit(3)
}
