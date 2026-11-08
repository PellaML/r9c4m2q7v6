import Foundation
import Metal

func question1(_ s: Session, _ mode: MTLStorageMode) throws {
    let parentUsage: MTLTextureUsage = [.shaderRead, .shaderWrite, .renderTarget, .pixelFormatView]
    try need(parentUsage.rawValue == 23, "q1_parent_usage_enum_contract")
    var creation: [String: [String: Any]] = [
        "candidate": ["status": "NOT_ATTEMPTED", "requested_parent_usage": 23],
        "direct_control": ["status": "NOT_ATTEMPTED", "requested_usage": 1,
            "legality": "independent native API unknown, not inferred from candidate view creation"]]
    func creationCheckpoint(_ stage: String) throws {
        s.report["creation_outcomes"] = creation; try s.checkpoint(stage)
    }
    try creationCheckpoint("q1_before_creation_prerequisites")
    let packed = try s.input("q1-patterns.bin", 1024)
    try need(packed.count == width * height * 3, "q1_pattern_length")
    let ar = s.dev.minimumLinearTextureAlignment(for: .r8Unorm)
    let aa = s.dev.minimumLinearTextureAlignment(for: .a8Unorm)
    s.report["alignment"] = ["r8": ar, "a8": aa]
    let directAlignmentValid = aa > 0 && aa <= 4096 && aa & (aa-1) == 0
    try need(ar > 0 && ar <= 4096 && ar & (ar-1) == 0, "r8_linear_alignment_unsupported_or_unbounded")
    // Invalid direct-A8 alignment must not preempt the R8->A8 view candidate.
    let rowPitch = max(256, max(ar, directAlignmentValid ? aa : ar))
    let offset = rowPitch, length = rowPitch * (height+2)
    s.report["requested"] = ["storage": mode.rawValue, "parent_format": "r8Unorm", "view_format": "a8Unorm",
        "type": "type2D", "mip_range": [0, 1], "slice_range": [0, 1], "swizzle": swizzleInfo(identity),
        "parent_usage": "shaderRead|shaderWrite|renderTarget|pixelFormatView", "parent_usage_raw": 23,
        "direct_control_usage": "shaderRead", "direct_control_usage_raw": 1,
        "width": width, "height": height,
        "pitch": rowPitch, "offset": offset, "length": length,
        "direct_control": "separate buffer allocation, directly created a8Unorm texture",
        "no_substitute_on_nil_or_remap": true]
    s.report["storage_scope"] = ["attempts": ["shared", "managed", "private"],
        "memoryless": "not a legal persistent MTLBuffer storage class; not attempted or counted supported",
        "execution": "each requested mode is a separate process; any refusal is not a pass"]
    s.report["input_patterns_sha256"] = sha(packed)
    let ops: [(String, String, Int)] = [("read-f32", "read_float", 32), ("read-f16", "read_half", 16),
        ("sample-f32", "sample_float", 32), ("sample-f16", "sample_half", 16)]
    var pipelines: [String: MTLComputePipelineState] = [:]
    for name in ops.map({ $0.1 }) + ["wrong_read_float"] {
        guard let f = s.library.makeFunction(name: name) else { throw Failure(reason: "q1_function_missing") }
        let p = try s.dev.makeComputePipelineState(function: f)
        try need(p.maxTotalThreadsPerThreadgroup >= 32, "compute_threadgroup_unsupported")
        pipelines[name] = p
    }
    var samplerInfo: [[String: Any]] = []
    func sampler(_ linear: Bool) throws -> MTLSamplerState {
        let d = MTLSamplerDescriptor()
        d.minFilter = linear ? .linear : .nearest; d.magFilter = d.minFilter; d.mipFilter = .notMipmapped
        d.normalizedCoordinates = true; d.sAddressMode = .clampToEdge; d.tAddressMode = .clampToEdge
        d.rAddressMode = .clampToEdge; d.maxAnisotropy = 1; d.lodMinClamp = 0; d.lodMaxClamp = 0
        samplerInfo.append(["linear": linear, "min": d.minFilter.rawValue, "mag": d.magFilter.rawValue,
            "mip": d.mipFilter.rawValue, "normalized": d.normalizedCoordinates,
            "address_str": [d.sAddressMode.rawValue, d.tAddressMode.rawValue, d.rAddressMode.rawValue],
            "anisotropy": d.maxAnisotropy, "lod_min": d.lodMinClamp, "lod_max": d.lodMaxClamp])
        guard let result = s.dev.makeSamplerState(descriptor: d) else { throw Failure(reason: "sampler_nil_unsupported") }
        return result
    }
    let nearest = try sampler(false), linear = try sampler(true)
    s.report["samplers_at_creation"] = samplerInfo
    var tag: UInt32 = 0
    func capture(_ t: MTLTexture, _ role: String, _ phase: Int, _ op: (String, String, Int),
                 _ negative: String = "") throws {
        tag += 1
        let name = "p\(phase)-\(role)-\(op.0)" + (negative.isEmpty ? "" : "-" + negative)
        let function = negative == "wrong-kernel" ? "wrong_read_float" : op.1
        let result = try s.buffer(traceGuard*2 + width*height*24)
        let cb = try s.command()
        guard let e = cb.makeComputeCommandEncoder(), let pso = pipelines[function] else {
            throw Failure(reason: "q1_encoder_or_pipeline_nil")
        }
        e.setComputePipelineState(pso); e.setTexture(t, index: 0)
        e.setSamplerState(negative == "wrong-state" ? linear : nearest, index: 0)
        e.setBuffer(result, offset: traceGuard, index: 0)
        var p = SIMD4<UInt32>(UInt32(width), UInt32(height), tag, 0)
        e.setBytes(&p, length: 16, index: 1)
        e.dispatchThreads(MTLSize(width: width, height: height, depth: 1),
                          threadsPerThreadgroup: MTLSize(width: 8, height: 4, depth: 1))
        e.endEncoding(); try s.finish(cb, name)
        try s.save(snapshot(result), ["kind": "values", "name": name, "phase": phase, "role": role,
            "operation": op.0, "precision": op.2, "kernel": function, "tag": tag,
            "negative": negative, "sampler": negative == "wrong-state" ? "linear" : "nearest",
            "actual_texture": textureInfo(t)])
    }
    func bytes(_ phase: Int, _ independent: Bool) throws -> Data {
        var d = Data(repeating: 0xa7, count: length)
        for y in 0..<height { for x in 0..<width {
            let value: UInt8
            if independent {
                if y == 0 && x == 0 { value = [UInt8(0), 255, 1][phase] }
                else if y == 0 && x == 1 { value = [UInt8(255), 0, 254][phase] }
                else { value = UInt8(truncatingIfNeeded: x*(37+3*y) + 61*y + 89*phase) }
            } else { value = packed[phase*width*height + y*width + x] }
            d[offset + y*rowPitch + x] = value
        } }
        return d
    }
    func baseTexture(_ b: MTLBuffer, _ format: MTLPixelFormat, _ role: String) throws -> MTLTexture {
        let d = MTLTextureDescriptor.texture2DDescriptor(pixelFormat: format, width: width, height: height, mipmapped: false)
        d.storageMode = mode
        d.usage = format == .r8Unorm ? parentUsage : [.shaderRead]
        d.hazardTrackingMode = .tracked
        // A normal buffer-backed texture, NOT a texture-buffer class or swizzle substitute.
        try s.checkpoint("before_" + role + "_buffer_texture")
        guard let t = b.makeTexture(descriptor: d, offset: offset, bytesPerRow: rowPitch) else {
            throw Failure(reason: role + "_buffer_texture_nil_unsupported_no_substitute")
        }
        var info = textureInfo(t)
        info["backing_buffer_storage"] = b.storageMode.rawValue
        info["backing_buffer_length"] = b.length
        info["backing_buffer_hazard_tracking"] = b.hazardTrackingMode.rawValue
        s.report[role] = info; try s.checkpoint(role + "_created")
        try need(t.pixelFormat == format && t.storageMode == mode && t.textureType == .type2D &&
            t.width == width && t.height == height && t.depth == 1 && t.mipmapLevelCount == 1 &&
            t.arrayLength == 1 && t.sampleCount == 1 && t.usage == d.usage &&
            t.usage.rawValue == (format == .r8Unorm ? 23 : 1) && t.buffer === b &&
            t.bufferOffset == offset && t.bufferBytesPerRow == rowPitch && t.parent == nil &&
            swizzleInfo(t.swizzle) == swizzleInfo(identity) && t.hazardTrackingMode == .tracked,
            role + "_actual_properties_or_storage_remap")
        return t
    }
    func view(_ parent: MTLTexture, _ b: MTLBuffer, _ role: String) throws -> MTLTexture {
        try s.checkpoint("before_" + role + "_a8_view")
        guard let t = parent.makeTextureView(pixelFormat: .a8Unorm, textureType: .type2D,
            levels: 0..<1, slices: 0..<1, swizzle: identity) else {
            throw Failure(reason: role + "_a8_view_nil_unsupported_no_substitute")
        }
        var info = textureInfo(t)
        info["parent_identity_matches"] = t.parent === parent
        info["backing_identity_matches"] = (t.buffer ?? t.parent?.buffer) === b
        s.report[role] = info; try s.checkpoint(role + "_created")
        try need(t.pixelFormat == .a8Unorm && t.storageMode == mode && t.textureType == .type2D &&
            t.width == width && t.height == height && t.depth == 1 && t.mipmapLevelCount == 1 &&
            t.arrayLength == 1 && t.sampleCount == 1 && t.usage == parentUsage && t.usage.rawValue == 23 &&
            t.parentRelativeLevel == 0 && t.parentRelativeSlice == 0 && t.parent === parent &&
            (t.buffer ?? t.parent?.buffer) === b && swizzleInfo(t.swizzle) == swizzleInfo(identity),
            role + "_view_properties_or_alias_mismatch")
        return t
    }
    // Only the tested view and independent A8 texture escape this autorelease pool.
    // Original R8, both explicit buffers, sibling view, and completed commands do not.
    let survivors = try autoreleasepool { () throws -> (MTLTexture, MTLTexture) in
        typealias Candidate = (buffer: MTLBuffer, parent: MTLTexture, view: MTLTexture, sibling: MTLTexture)
        var candidate: Candidate?
        var control: (buffer: MTLBuffer, texture: MTLTexture)?
        // Candidate and control legality are separate unknowns. Catchable nil/API
        // failures in either do not prevent attempting the other; never substitute.
        var stage = "buffer"
        creation["candidate"]?["status"] = "ATTEMPTING"
        try creationCheckpoint("q1_candidate_creation_start")
        do {
            let b = try s.buffer(length, mode)
            stage = "r8_parent_usage23"
            let parent = try baseTexture(b, .r8Unorm, "parent_r8")
            stage = "a8_identity_view"
            let tested = try view(parent, b, "view_a8")
            stage = "a8_sibling_view"
            let alias = try view(parent, b, "sibling_a8")
            candidate = (b, parent, tested, alias)
            creation["candidate"]?["status"] = "CREATED_ACTUAL_PROPERTIES_VERIFIED"
        } catch {
            creation["candidate"]?["status"] = "FAILED_NO_SUBSTITUTE"
            creation["candidate"]?["failure_stage"] = stage
            creation["candidate"]?["failure"] = errorInfo(error)
        }
        try creationCheckpoint("q1_candidate_creation_finished")
        stage = "a8_linear_alignment"
        creation["direct_control"]?["status"] = "ATTEMPTING"
        try creationCheckpoint("q1_direct_control_creation_start")
        do {
            try need(directAlignmentValid, "direct_a8_linear_alignment_unsupported_or_unbounded")
            stage = "buffer"
            let b = try s.buffer(length, mode)
            stage = "direct_a8_buffer_texture_shaderRead"
            let texture = try baseTexture(b, .a8Unorm, "direct_a8")
            control = (b, texture)
            creation["direct_control"]?["status"] = "CREATED_ACTUAL_PROPERTIES_VERIFIED"
        } catch {
            creation["direct_control"]?["status"] = "FAILED_NO_SUBSTITUTE"
            creation["direct_control"]?["failure_stage"] = stage
            creation["direct_control"]?["failure"] = errorInfo(error)
        }
        s.report["comparison_ready"] = candidate != nil && control != nil
        try creationCheckpoint("q1_independent_creation_outcomes")
        guard let c = candidate, let direct = control else {
            throw Failure(reason: "q1_creation_not_qualified_see_separate_candidate_and_control_outcomes")
        }
        let rb = c.buffer, r = c.parent, v = c.view, sibling = c.sibling
        let ab = direct.buffer, a = direct.texture
        try need(rb !== ab, "independent_control_allocation")
        for phase in 0..<3 {
            let rd = try bytes(phase, false), ad = try bytes(phase, true)
            try need(rd == ad, "independently_constructed_pattern_mismatch")
            if phase == 0 {
                let ru = try s.uploaded(rd), au = try s.uploaded(ad)
                try s.blit("p\(phase)-upload") {
                    $0.copy(from: ru, sourceOffset: 0, to: rb, destinationOffset: 0, size: length)
                    $0.copy(from: au, sourceOffset: 0, to: ab, destinationOffset: 0, size: length)
                }
                try need(snapshot(ru) == rd && snapshot(au) == ad, "q1_upload_input_changed")
            } else {
                func mutate(_ b: MTLBuffer, _ data: Data, _ name: String) throws {
                    if phase == 1 && mode != .private {
                        data.withUnsafeBytes { b.contents().copyMemory(from: $0.baseAddress!, byteCount: $0.count) }
                        if mode == .managed { b.didModifyRange(0..<length) }
                        s.report["phase1_write"] = "direct CPU buffer write; managed didModifyRange when requested"
                    } else {
                        let upload = try s.uploaded(data)
                        try s.blit("p\(phase)-\(name)-upload") {
                            $0.copy(from: upload, sourceOffset: 0, to: b, destinationOffset: 0, size: length)
                        }
                        try need(snapshot(upload) == data, "q1_mutation_upload_changed")
                        if phase == 1 { s.report["phase1_write"] = "private buffer: CPU staging plus real GPU copy, not direct CPU access" }
                    }
                }
                try mutate(rb, rd, "r")
                // One-sided mutation: aliases must change while the independent
                // direct A8 allocation must still contain the preceding pattern.
                try capture(v, "updated-only-view", phase, ops[0])
                try capture(a, "old-direct", phase, ops[0])
                try mutate(ab, ad, "a")
            }
            for (role, t) in [("view", v), ("sibling", sibling), ("direct", a), ("r8-negative", r)] {
                for op in ops { try capture(t, role, phase, op) }
            }
            if phase == 0 {
                try capture(v, "view", phase, ops[0], "wrong-kernel")
                try capture(r, "view", phase, ops[0], "wrong-output")
                try capture(v, "view", phase, ops[2], "wrong-state")
            }
            let rr = try s.buffer(length), ar = try s.buffer(length)
            // GPU copies observe managed/private bytes without unsynchronized CPU reads.
            try s.blit("p\(phase)-backing-readback") {
                $0.copy(from: rb, sourceOffset: 0, to: rr, destinationOffset: 0, size: length)
                $0.copy(from: ab, sourceOffset: 0, to: ar, destinationOffset: 0, size: length)
            }
            for (role, result, expected) in [("r8", rr, rd), ("a8", ar, ad)] {
                let actual = snapshot(result)
                try s.save(actual, ["kind": "backing", "name": "p\(phase)-backing-\(role)",
                    "phase": phase, "role": role, "pitch": rowPitch, "buffer_offset": offset])
                try need(actual == expected, "q1_backing_or_padding_changed")
            }
        }
        return (v, a)
    }
    s.report["lifetime"] = ["explicit_original_owners_released": true, "autorelease_pool_drained": true,
        "prior_commands_completed": true, "only_surviving_view_and_independent_control_escaped": true,
        "not_a_claim_of_underlying_allocation_deallocation": true]
    for (role, t) in [("view", survivors.0), ("direct", survivors.1)] {
        for op in ops { try capture(t, role, 3, op) }
    }
    try need(tag == 63 && s.records.count == 69, "q1_coverage_incomplete")
    s.report["value_dispatches"] = tag
}
