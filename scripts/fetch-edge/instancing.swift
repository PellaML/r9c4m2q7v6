// Native Metal API bridge only; expectations and acceptance live in Rust.
import Foundation
import Metal
import CryptoKit
import Darwin

struct Failure: Error { let reason: String }
func need(_ ok: Bool, _ why: String) throws { if !ok { throw Failure(reason: why) } }
func sha(_ data: Data) -> String { SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() }
func details(_ error: Error) -> [String: Any] {
    if let f = error as? Failure { return ["reason": f.reason] }
    let e = error as NSError
    return ["domain": String(e.domain.prefix(128)), "code": e.code,
            "description": String(e.localizedDescription.prefix(2048))]
}
func osBuild() throws -> String {
    var n = 0
    try need(sysctlbyname("kern.osversion", nil, &n, nil, 0) == 0 && n > 1 && n < 128, "os_build_size")
    var b = [CChar](repeating: 0, count: n)
    let rc = b.withUnsafeMutableBytes { sysctlbyname("kern.osversion", $0.baseAddress, &n, nil, 0) }
    try need(rc == 0, "os_build_unavailable")
    return String(cString: b)
}
let args = CommandLine.arguments
guard args.count == 3 else { exit(64) }
let root = URL(fileURLWithPath: args[2], isDirectory: true)
// Refuse a repeated native invocation without replacing earlier receipts.
guard !["native.tsv", "native-baseline.json", "native-status.txt"].contains(where: {
    FileManager.default.fileExists(atPath: root.appendingPathComponent($0).path)
}) else { exit(64) }
var info: [String: Any] = ["schema": 1, "scope": "actual native runtime only; no cross-runtime equivalence"]
var status = "FAILED"
var rows = "id\ttopology\tinstances\theight\tmode\tsubmitted\twidth\tformat\tsamples\tvertices_or_indices\tbase_instance\tcommitted\twait_returned\tcommand_status\terror_present\traw_bytes\n"
func flush() throws {
    info["status"] = status
    let data = try JSONSerialization.data(withJSONObject: info, options: [.prettyPrinted, .sortedKeys])
    try need(data.count <= 65536, "metadata_budget")
    try data.write(to: root.appendingPathComponent("native-baseline.json"), options: .atomic)
    try rows.write(to: root.appendingPathComponent("native.tsv"), atomically: true, encoding: .utf8)
    try (status + "\n").write(to: root.appendingPathComponent("native-status.txt"), atomically: true, encoding: .utf8)
}
do {
    info["os_version_and_build"] = ProcessInfo.processInfo.operatingSystemVersionString
    info["os_build"] = try osBuild()
    guard let framework = Bundle(path: "/System/Library/Frameworks/Metal.framework"),
          let version = framework.object(forInfoDictionaryKey: "CFBundleVersion") as? String,
          !version.isEmpty else { throw Failure(reason: "Metal_framework_version_unavailable") }
    info["Metal_framework"] = ["identifier": framework.bundleIdentifier ?? "unavailable", "build": version,
        "short_version": framework.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unavailable"]
    guard let device = MTLCreateSystemDefaultDevice() else { throw Failure(reason: "native_Metal_device_unavailable") }
    info["device"] = ["name": device.name, "registry_id": String(device.registryID),
                      "unified_memory": device.hasUnifiedMemory, "low_power": device.isLowPower,
                      "removable": device.isRemovable, "mac2": device.supportsFamily(.mac2)]
    try flush()
    guard let queue = device.makeCommandQueue() else { throw Failure(reason: "native_queue_unavailable") }
    let source = try Data(contentsOf: URL(fileURLWithPath: args[1]))
    guard let text = String(data: source, encoding: .utf8) else { throw Failure(reason: "MSL_not_UTF8") }
    let options = MTLCompileOptions(); options.fastMathEnabled = false
    info["MSL_sha256"] = sha(source)
    info["compiler"] = ["fast_math": options.fastMathEnabled, "language_version": options.languageVersion.rawValue]
    let library = try device.makeLibrary(source: text, options: options)
    let p = MTLRenderPipelineDescriptor()
    p.vertexFunction = library.makeFunction(name: "row_vertex")
    p.fragmentFunction = library.makeFunction(name: "row_fragment")
    p.colorAttachments[0].pixelFormat = .rgba32Float
    p.colorAttachments[0].writeMask = .all; p.colorAttachments[0].isBlendingEnabled = false
    p.rasterSampleCount = 1; p.depthAttachmentPixelFormat = .invalid; p.stencilAttachmentPixelFormat = .invalid
    let pipeline = try device.makeRenderPipelineState(descriptor: p)
    info["pipeline"] = ["format": p.colorAttachments[0].pixelFormat.rawValue, "samples": p.rasterSampleCount,
        "write_mask": p.colorAttachments[0].writeMask.rawValue, "blending": p.colorAttachments[0].isBlendingEnabled,
        "depth": p.depthAttachmentPixelFormat.rawValue, "stencil": p.stencilAttachmentPixelFormat.rawValue]
    // Separate owned index streams; no vertex buffers or external geometry.
    func indexBuffer(_ indices: [UInt16]) throws -> MTLBuffer {
        guard let b = indices.withUnsafeBytes({ device.makeBuffer(bytes: $0.baseAddress!, length: $0.count, options: .storageModeShared) })
        else { throw Failure(reason: "index_allocation") }; return b
    }
    let strip = try indexBuffer([0, 1, 2, 3]), list = try indexBuffer([0, 1, 2, 2, 1, 3])
    var id = 0
    status = "ACQUIRING"; try flush()
    for topology in 0..<3 {
        for count in [1, 64, 65, 108, 128, 4096] {
            var height = 128
            while height <= count { height *= 2 } // Always retain at least one untouched row.
            for mode in (count > 64 ? [0, 1, 2] : [0]) {
                let submitted = mode == 1 ? min(count, 64) : count
                let desc = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: .rgba32Float, width: 1, height: height, mipmapped: false)
                desc.storageMode = .private; desc.usage = .renderTarget
                let length = 512 + 256 * height
                guard let texture = device.makeTexture(descriptor: desc),
                      let readback = device.makeBuffer(length: length, options: .storageModeShared)
                else { throw Failure(reason: "native_resource_allocation") }
                try need(texture.width == 1 && texture.height == height && texture.pixelFormat == .rgba32Float &&
                         texture.sampleCount == 1 && texture.storageMode == .private && !texture.isFramebufferOnly,
                         "native_texture_contract")
                memset(readback.contents(), 0xcd, length)
                let cd = MTLCommandBufferDescriptor(); cd.errorOptions = .encoderExecutionStatus
                guard let cb = queue.makeCommandBuffer(descriptor: cd) else { throw Failure(reason: "command_allocation") }
                cb.label = "owned-row-\(id)"
                let pass = MTLRenderPassDescriptor()
                pass.renderTargetWidth = 1; pass.renderTargetHeight = height
                pass.colorAttachments[0].texture = texture
                pass.colorAttachments[0].loadAction = .clear; pass.colorAttachments[0].storeAction = .store
                pass.colorAttachments[0].clearColor = MTLClearColorMake(-1, -2, -4, -8)
                guard let enc = cb.makeRenderCommandEncoder(descriptor: pass) else { throw Failure(reason: "render_encoder") }
                enc.label = "owned-rows"; enc.setRenderPipelineState(pipeline); enc.setCullMode(.none)
                enc.setViewport(MTLViewport(originX: 0, originY: 0, width: 1, height: Double(height), znear: 0, zfar: 1))
                enc.setScissorRect(MTLScissorRect(x: 0, y: 0, width: 1, height: height))
                let params = [UInt32(height), UInt32(mode == 2 ? 1 : 0)]
                params.withUnsafeBytes { enc.setVertexBytes($0.baseAddress!, length: $0.count, index: 0) }
                if topology == 0 {
                    enc.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4, instanceCount: submitted, baseInstance: 0)
                } else {
                    enc.drawIndexedPrimitives(type: topology == 1 ? .triangleStrip : .triangle, indexCount: topology == 1 ? 4 : 6,
                        indexType: .uint16, indexBuffer: topology == 1 ? strip : list, indexBufferOffset: 0,
                        instanceCount: submitted, baseVertex: 0, baseInstance: 0)
                }
                enc.endEncoding()
                guard let blit = cb.makeBlitCommandEncoder() else { throw Failure(reason: "blit_encoder") }
                blit.label = "owned-readback"
                blit.copy(from: texture, sourceSlice: 0, sourceLevel: 0, sourceOrigin: MTLOrigin(x: 0, y: 0, z: 0),
                          sourceSize: MTLSize(width: 1, height: height, depth: 1), to: readback, destinationOffset: 256,
                          destinationBytesPerRow: 256, destinationBytesPerImage: 256 * height)
                blit.endEncoding()
                info["active_case"] = id; try flush()
                cb.commit(); cb.waitUntilCompleted()
                // Save ALL buffer bytes, even after a command failure; never fix or compact them.
                let raw = Data(bytes: readback.contents(), count: length)
                try raw.write(to: root.appendingPathComponent(String(format: "case%02d.bin", id)), options: .withoutOverwriting)
                rows += "\(id)\t\(topology)\t\(count)\t\(height)\t\(mode)\t\(submitted)\t\(texture.width)\t\(texture.pixelFormat.rawValue)\t\(texture.sampleCount)\t\(topology == 2 ? 6 : 4)\t0\t1\t1\t\(cb.status.rawValue)\t\(cb.error == nil ? 0 : 1)\t\(raw.count)\n"
                if let error = cb.error { info["command_error"] = details(error) }
                try flush()
                try need(cb.status == .completed && cb.error == nil, "command_not_cleanly_completed")
                id += 1
            }
        }
    }
    try need(id == 42, "case_count")
    info["captured_cases"] = id; status = "CAPTURED_ALL"; try flush()
} catch {
    status = "FAILED"; info["failure"] = details(error)
    do { try flush() } catch { fputs("native receipt write failed\n", stderr) }
    exit(3)
}
