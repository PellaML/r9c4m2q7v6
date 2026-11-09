// Native-only API bridge. No runtime MSL compilation, arithmetic oracle or CPU
// fallback. Unsafe byte access is confined to checked native buffer transfers.
import Foundation
import Metal
import CryptoKit
import Dispatch

struct Failure: Error { let reason: String }
func need(_ ok: Bool, _ reason: String) throws { if !ok { throw Failure(reason: reason) } }
func sha(_ bytes: Data) -> String { SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined() }
func csv(_ values: [UInt]) -> String { values.map(String.init).joined(separator: ",") }
struct Spec: Decodable {
    let name: String, kind: String
    let bits: Int, mask: UInt, tag: UInt32, negative: Bool
    let clearBits: String
}
struct Plan: Decodable {
    let schema: Int, rows: Int, width: Int, height: Int, pitch: Int, guardBytes: Int, traceBytes: Int, readBytes: Int
    let captures: [Spec]
}
guard CommandLine.arguments.count == 3 && ["precise", "fast"].contains(CommandLine.arguments[2]) else { exit(2) }
let mode = CommandLine.arguments[2]
let modeIndex: UInt32 = mode == "precise" ? 0 : 1
let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
var receipt = "name\ttag\tlibrary_sha256\tinputs_sha256\ttexture\tpipeline\tpass\tclear_bits\tstatus\terror\tinputs_unchanged\tdraws\tread_dispatches\tgpu_start\tgpu_end\traw_sha256\ttrace_sha256\tread_sha256\n"
var report: [String: Any] = ["schema": 1, "status": "UNQUALIFIED", "cpu_fallback": false]
func read(_ name: String) throws -> Data {
    let path = root.appendingPathComponent(name)
    let attributes = try FileManager.default.attributesOfItem(atPath: path.path)
    let length = (attributes[.size] as? NSNumber)?.intValue ?? Int.max
    try need(attributes[.type] as? FileAttributeType == .typeRegular && length <= 1_048_576, "input type/size")
    let bytes = try Data(contentsOf: path)
    try need(bytes.count <= 1_048_576, "input byte bound")
    return bytes
}
func save(_ name: String, _ bytes: Data) throws {
    try need(!name.contains("/") && !name.contains("\\") && bytes.count <= 1_048_576, "output name/size")
    try bytes.write(to: root.appendingPathComponent(name), options: .withoutOverwriting)
}
func run() throws {
    var endian: UInt16 = 1
    try need(withUnsafeBytes(of: &endian) { $0[0] == 1 }, "little endian required")
    let planData = try read("plan.json"), sourceData = try read("source.bin"), seedData = try read("seed.bin")
    let m = try JSONDecoder().decode(Plan.self, from: planData)
    try need(m.schema == 2 && m.rows == 64 && m.width == 3 && m.height == 66 && m.pitch == 256 &&
             m.guardBytes == 256 && m.traceBytes == 2176 && m.readBytes == 10016 && m.captures.count == 21 &&
             sourceData.count == 1024 && seedData.count == 17408, "fixed plan geometry")
    let names = ["seed", "clear", "clear-zero", "clear-negative-zero", "load", "f32-replace", "f16-replace",
                 "f32-none", "f16-none", "f32-blend", "f16-blend", "f32-red", "f32-absent", "negative-skip", "clear-tie", "clear-tie-below", "clear-tie-above", "clear-subnormal", "clear-underflow", "clear-overflow", "clear-odd-tie"]
    let kinds = ["seed", "clear", "clear-zero", "clear-negative-zero", "load", "replace", "replace",
                 "replace", "replace", "blend", "blend", "replace", "replace", "replace", "clear-probe", "clear-probe", "clear-probe", "clear-probe", "clear-probe", "clear-probe", "clear-probe"]
    let widths = [0, 0, 0, 0, 0, 32, 16, 32, 16, 32, 16, 32, 32, 32, 0, 0, 0, 0, 0, 0, 0]
    let masks: [UInt] = [0, 0, 0, 0, 0, 15, 15, 0, 0, 15, 15, 8, 7, 15, 0, 0, 0, 0, 0, 0, 0]
    for (i, s) in m.captures.enumerated() {
        try need(s.name == names[i] && s.kind == kinds[i] && s.bits == widths[i] && s.mask == masks[i] &&
                 s.tag == UInt32(i + 1) && s.negative == (i == 13) && s.clearBits.count == 16 && UInt64(s.clearBits, radix: 16) != nil, "fixed plan state/order")
    }
    var inputs = planData; inputs.append(sourceData); inputs.append(seedData)
    let inputHash = sha(inputs)
    let libraryBytes = try read("program.metallib")
    try need(Array(libraryBytes.prefix(4)) == [0x4d, 0x54, 0x4c, 0x42], "metallib signature")
    let loadedHash = sha(libraryBytes)
    guard let dev = MTLCreateSystemDefaultDevice(), let queue = dev.makeCommandQueue() else {
        throw Failure(reason: "native Metal device/queue unavailable")
    }
    // The hash is computed on the SAME Data supplied to makeLibrary(data:).
    let library = try dev.makeLibrary(data: libraryBytes.withUnsafeBytes { DispatchData(bytes: $0) })
    try need(Set(library.functionNames) == Set(["triangle", "emit_f32", "emit_f16", "observe", "arithmetic"]), "exact entry set")
    guard let vertex = library.makeFunction(name: "triangle"), let f32 = library.makeFunction(name: "emit_f32"),
          let f16 = library.makeFunction(name: "emit_f16"), let kernel = library.makeFunction(name: "observe"), let arithmetic = library.makeFunction(name: "arithmetic") else {
        throw Failure(reason: "missing function")
    }
    try need(vertex.functionType == .vertex && f32.functionType == .fragment && f16.functionType == .fragment &&
             kernel.functionType == .kernel && arithmetic.functionType == .kernel, "stage types")
    let compute = try dev.makeComputePipelineState(function: kernel)
    let mathPSO = try dev.makeComputePipelineState(function: arithmetic)
    try need(mathPSO.maxTotalThreadsPerThreadgroup >= 32, "arithmetic threadgroup capacity")
    try need(compute.maxTotalThreadsPerThreadgroup >= 16, "read threadgroup capacity")
    report["device"] = dev.name // No serial, registry identifier, hostname, or address.
    report["os"] = ProcessInfo.processInfo.operatingSystemVersionString
    report["unified_memory"] = dev.hasUnifiedMemory
    report["loaded_sha256"] = loadedHash
    report["inputs_sha256"] = inputHash
    report["math_mode"] = mode + "; actual compiler flags/AIR retained; no assumed contraction"
    report["guard_scope"] = "host-visible readback prefix/suffix/stride and untouched texels; NOT opaque private allocation padding"
    report["texture_columns"] = "format,storage,usage,width,height,depth,mips,samples,array,type,swizzle_rgba,buffer_backed,framebuffer_only,hazard"
    report["pipeline_columns"] = "mask,blend,src_rgb,dst_rgb,rgb_op,src_alpha,dst_alpha,alpha_op,samples,alpha_to_coverage,alpha_to_one,rasterization"
    report["pass_columns"] = "load,store,level,slice,depth_plane,resolve_attached"

    func buffer(_ length: Int) throws -> MTLBuffer {
        guard let b = dev.makeBuffer(length: length, options: .storageModeShared) else { throw Failure(reason: "buffer allocation") }
        try need(b.length == length && b.storageMode == .shared, "actual shared buffer")
        memset(b.contents(), 0xcd, b.length)
        return b
    }
    func snapshot(_ b: MTLBuffer) -> Data { Data(bytes: b.contents(), count: b.length) }
    let source = try buffer(sourceData.count + 128), upload = try buffer(seedData.count)
    sourceData.withUnsafeBytes { source.contents().advanced(by: 64).copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    seedData.withUnsafeBytes { upload.contents().copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
    let sourceBefore = snapshot(source), uploadBefore = snapshot(upload)
    let size = MTLSize(width: m.width, height: m.height, depth: 1), origin = MTLOrigin(x: 0, y: 0, z: 0)
    var completed = 0, draws = 0
    for s in m.captures {
        if modeIndex == 1 && s.name == "clear-overflow" { continue } // Not a fast-math nonfinite qualification.
        let tag = s.tag | (modeIndex << 8)
        let td = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .r16Float, width: m.width, height: m.height, mipmapped: false)
        td.storageMode = .private; td.usage = [.shaderRead, .renderTarget]
        td.hazardTrackingMode = .tracked
        td.swizzle = MTLTextureSwizzleChannels(red: .red, green: .green, blue: .blue, alpha: .alpha)
        guard let t = dev.makeTexture(descriptor: td) else { throw Failure(reason: "R16Float target unavailable; no substitute") }
        let sw = t.swizzle
        try need(t.pixelFormat == .r16Float && t.storageMode == .private && t.usage == [.shaderRead, .renderTarget] &&
                 t.textureType == .type2D && t.width == 3 && t.height == 66 && t.depth == 1 &&
                 t.mipmapLevelCount == 1 && t.sampleCount == 1 && t.arrayLength == 1 &&
                 sw.red == .red && sw.green == .green && sw.blue == .blue && sw.alpha == .alpha &&
                 t.buffer == nil && !t.isFramebufferOnly && t.hazardTrackingMode == .tracked, "actual texture mismatch")
        let textureState = csv([t.pixelFormat.rawValue, t.storageMode.rawValue, t.usage.rawValue,
            UInt(t.width), UInt(t.height), UInt(t.depth), UInt(t.mipmapLevelCount), UInt(t.sampleCount), UInt(t.arrayLength),
            t.textureType.rawValue, UInt(sw.red.rawValue), UInt(sw.green.rawValue), UInt(sw.blue.rawValue), UInt(sw.alpha.rawValue),
            t.buffer == nil ? 0 : 1, t.isFramebufferOnly ? 1 : 0, t.hazardTrackingMode.rawValue])
        var pipelineState = "none", passState = "none", clearBits = "none"
        var render: MTLRenderPipelineState? = nil
        if s.bits != 0 {
            let pd = MTLRenderPipelineDescriptor()
            pd.vertexFunction = vertex; pd.fragmentFunction = s.bits == 32 ? f32 : f16
            pd.rasterSampleCount = 1; pd.isAlphaToCoverageEnabled = false; pd.isAlphaToOneEnabled = false; pd.isRasterizationEnabled = true
            let a = pd.colorAttachments[0]!
            a.pixelFormat = .r16Float; a.writeMask = MTLColorWriteMask(rawValue: s.mask)
            a.isBlendingEnabled = s.kind == "blend"
            a.rgbBlendOperation = .add; a.alphaBlendOperation = .add
            a.sourceRGBBlendFactor = .one
            a.destinationRGBBlendFactor = s.kind == "blend" ? .oneMinusSourceColor : .zero
            a.sourceAlphaBlendFactor = s.kind == "blend" ? .zero : .one
            a.destinationAlphaBlendFactor = .zero
            pipelineState = csv([a.writeMask.rawValue, a.isBlendingEnabled ? 1 : 0,
                a.sourceRGBBlendFactor.rawValue, a.destinationRGBBlendFactor.rawValue, a.rgbBlendOperation.rawValue,
                a.sourceAlphaBlendFactor.rawValue, a.destinationAlphaBlendFactor.rawValue, a.alphaBlendOperation.rawValue,
                UInt(pd.rasterSampleCount), pd.isAlphaToCoverageEnabled ? 1 : 0, pd.isAlphaToOneEnabled ? 1 : 0, pd.isRasterizationEnabled ? 1 : 0])
            // Descriptor queried immediately before compilation, not opaque PSO introspection.
            render = try dev.makeRenderPipelineState(descriptor: pd)
        }
        let raw = try buffer(seedData.count), trace = try buffer(m.traceBytes), reads = try buffer(m.readBytes)
        guard let cb = queue.makeCommandBuffer(), let up = cb.makeBlitCommandEncoder() else { throw Failure(reason: "upload command") }
        cb.label = s.name
        up.copy(from: upload, sourceOffset: m.guardBytes, sourceBytesPerRow: m.pitch, sourceBytesPerImage: m.pitch * m.height,
                sourceSize: size, to: t, destinationSlice: 0, destinationLevel: 0, destinationOrigin: origin)
        up.endEncoding()
        if s.kind != "seed" {
            let pass = MTLRenderPassDescriptor()
            let a = pass.colorAttachments[0]!
            a.texture = t; a.level = 0; a.slice = 0; a.depthPlane = 0
            a.loadAction = s.kind.hasPrefix("clear") ? .clear : .load; a.storeAction = .store
            let red = Double(bitPattern: UInt64(s.clearBits, radix: 16)!)
            a.clearColor = MTLClearColorMake(red, 2.0, -4.0, 0.125)
            passState = csv([a.loadAction.rawValue, a.storeAction.rawValue, UInt(a.level), UInt(a.slice), UInt(a.depthPlane), a.resolveTexture == nil ? 0 : 1])
            clearBits = String(format: "%016llx", a.clearColor.red.bitPattern)
            guard let e = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(reason: "render encoder") }
            if let render = render {
                e.setRenderPipelineState(render); e.setCullMode(.none); e.setTriangleFillMode(.fill)
                e.setViewport(MTLViewport(originX: 0, originY: 0, width: Double(m.width), height: Double(m.height), znear: 0, zfar: 1))
                e.setScissorRect(MTLScissorRect(x: 1, y: 1, width: 1, height: m.rows))
                e.setFragmentBuffer(source, offset: 64, index: 0); e.setFragmentBuffer(trace, offset: 64, index: 1)
                var params = SIMD4<UInt32>(UInt32(m.rows), tag, s.negative ? 0 : UInt32.max, 0)
                e.setFragmentBytes(&params, length: 16, index: 2)
                e.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3); draws += 1
            }
            e.endEncoding()
        }
        guard let c = cb.makeComputeCommandEncoder() else { throw Failure(reason: "read encoder") }
        c.setComputePipelineState(compute); c.setTexture(t, index: 0); c.setTexture(t, index: 1)
        c.setBuffer(reads, offset: 64, index: 0)
        var params = SIMD4<UInt32>(UInt32(m.width), UInt32(m.height), tag, 0)
        c.setBytes(&params, length: 16, index: 1)
        c.dispatchThreads(MTLSize(width: 4, height: m.height + 2, depth: 1), threadsPerThreadgroup: MTLSize(width: 4, height: 4, depth: 1))
        c.endEncoding()
        guard let down = cb.makeBlitCommandEncoder() else { throw Failure(reason: "readback encoder") }
        down.copy(from: t, sourceSlice: 0, sourceLevel: 0, sourceOrigin: origin, sourceSize: size,
                  to: raw, destinationOffset: m.guardBytes, destinationBytesPerRow: m.pitch, destinationBytesPerImage: m.pitch * m.height)
        down.endEncoding()
        let completion = DispatchSemaphore(value: 0)
        cb.addCompletedHandler { _ in completion.signal() }
        cb.commit()
        try need(completion.wait(timeout: .now() + 10) == .success, "GPU timeout; no retry")
        let unchanged = snapshot(source) == sourceBefore && snapshot(upload) == uploadBefore
        report["last_command"] = ["name": s.name, "status": cb.status.rawValue, "error_present": cb.error != nil, "inputs_unchanged": unchanged]
        try need(cb.status == .completed && cb.error == nil && unchanged, "GPU failure or modified input guards")
        let rawData = snapshot(raw), traceData = snapshot(trace), readData = snapshot(reads)
        // Only real, completed GPU bytes are saved; no patching or canonicalization.
        try save(s.name + "-raw.bin", rawData); try save(s.name + "-trace.bin", traceData); try save(s.name + "-read.bin", readData)
        receipt += [s.name, String(tag), loadedHash, inputHash, textureState, pipelineState, passState, clearBits,
            String(cb.status.rawValue), cb.error == nil ? "0" : "1", unchanged ? "1" : "0", s.bits == 0 ? "0" : "1", "1",
            String(cb.gpuStartTime), String(cb.gpuEndTime), sha(rawData), sha(traceData), sha(readData)].joined(separator: "\t") + "\n"
        completed += 1
    }
    try need(completed == (modeIndex == 0 ? 21 : 20) && draws == 9, "capture coverage")
    var mathReceipts = "mode\tprecision\tlibrary_sha256\tinputs_sha256\tstatus\terror\tinputs_unchanged\tgrid\tthreads_per_group\tgpu_start\tgpu_end\tsha256\n"
    for bits in [UInt32(32), UInt32(16)] {
        let output = try buffer(6656)
        guard let cb = queue.makeCommandBuffer(), let e = cb.makeComputeCommandEncoder() else { throw Failure(reason: "arithmetic command") }
        e.setComputePipelineState(mathPSO)
        e.setBuffer(source, offset: 64, index: 0); e.setBuffer(upload, offset: m.guardBytes, index: 1)
        e.setBuffer(output, offset: 64, index: 2)
        var p = SIMD4<UInt32>(UInt32(m.rows), bits, 0x6000 | (modeIndex << 8) | bits, 0)
        e.setBytes(&p, length: 16, index: 3)
        e.dispatchThreads(MTLSize(width: m.rows + 4, height: 1, depth: 1), threadsPerThreadgroup: MTLSize(width: 32, height: 1, depth: 1))
        e.endEncoding()
        let done = DispatchSemaphore(value: 0); cb.addCompletedHandler { _ in done.signal() }; cb.commit()
        try need(done.wait(timeout: .now() + 10) == .success, "arithmetic GPU timeout; no retry")
        let unchanged = snapshot(source) == sourceBefore && snapshot(upload) == uploadBefore
        try need(cb.status == .completed && cb.error == nil && unchanged, "arithmetic command failure")
        let bytes = snapshot(output)
        try save("math-f\(bits).bin", bytes)
        mathReceipts += [mode, String(bits), loadedHash, inputHash, String(cb.status.rawValue), cb.error == nil ? "0" : "1",
            unchanged ? "1" : "0", String(m.rows + 4), "32", String(cb.gpuStartTime), String(cb.gpuEndTime), sha(bytes)].joined(separator: "\t") + "\n"
    }
    try save("math.tsv", Data(mathReceipts.utf8))
    report["command_buffers"] = completed + 2; report["render_passes"] = completed - 1; report["draws"] = draws; report["read_dispatches"] = completed
    report["arithmetic_dispatches"] = 2
    report["status"] = "NATIVE_BYTES_CAPTURED_MATH_PROFILE_UNSELECTED"
}
var code: Int32 = 0
do { try run() } catch {
    if let error = error as? Failure { report["failure"] = error.reason }
    else { let e = error as NSError; report["failure"] = ["domain": e.domain, "code": e.code] }
    code = 3
}
do {
    try save("gpu.tsv", Data(receipt.utf8))
    try save("gpu.json", JSONSerialization.data(withJSONObject: report, options: [.sortedKeys, .prettyPrinted]))
} catch { code = 4 }
exit(code)
