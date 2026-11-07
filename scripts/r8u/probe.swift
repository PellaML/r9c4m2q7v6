// Newly authored synthetic inputs. Public Metal API; no application/guest inputs.
import Foundation
import Metal
import CryptoKit
struct Failure: Error { let message:String }
func need(_ b:Bool,_ s:String)throws{if !b{throw Failure(message:s)}}
func data<T>(_ a:[T])->Data{a.withUnsafeBytes{Data($0)}}
func sha(_ d:Data)->String{SHA256.hash(data:d).map{String(format:"%02x",$0)}.joined()}
var rng:UInt64=0x3141592653589793
func next()->UInt32{rng=rng &* 6364136223846793005 &+ 1442695040888963407;return UInt32(truncatingIfNeeded:rng>>32)}
let smallSeed=(0..<256).map{UInt8(($0*73+19)&255)}
var uv:[SIMD2<Float>]=[];var coordSets:[[String:Any]]=[]
func coordSet(_ name:String,_ body:()->Void){let start=uv.count;body();coordSets.append(["name":name,"start":start,"count":uv.count-start])}
coordSet("centers"){for y in 0..<16{for x in 0..<16{uv.append(SIMD2((Float(x)+0.5)/16,(Float(y)+0.5)/16))}}}
coordSet("dense_grid"){for y in 0...136{for x in 0...136{uv.append(SIMD2((Float(x)/8-0.5)/16,(Float(y)/8-0.5)/16))}}}
coordSet("weight_ties"){for base in [0,6,14]{for k in 0..<256{let u=(Float(base)+0.5+(Float(k)+0.5)/256)/16;for x in [u.nextDown,u,u.nextUp]{uv.append(SIMD2(x,7.5/16));uv.append(SIMD2(7.5/16,x))}}}}
coordSet("edges"){var v:[Float]=[-1e30,-1,-0.0,0,1e-30,1,2,1e30];for k in 0...16{let u=Float(k)/16;v += [u.nextDown,u,u.nextUp]};for y in v{for x in v{uv.append(SIMD2(x,y))}}}
coordSet("random"){for _ in 0..<4096{let x=Float(next()>>8)/16777216*1.25-0.125;let y=Float(next()>>8)/16777216*1.25-0.125;uv.append(SIMD2(x,y))}}
let BW=256,BH=256,BN=65536
let bigSeed=(0..<BN).map{UInt8($0/BW)}
let anchors:[Float]=[-2,-1,-0.0001,-0.0,0,Float.leastNonzeroMagnitude,0.0001,1/255,0.125,0.5,0.875,1-Float.ulpOfOne,1,Float(1).nextUp,1.25,2]
var sources:[[SIMD4<Float>]]=[]
for ds in 0..<2 {
    var a:[SIMD4<Float>]=[]
    for y in 0..<BH{for x in 0..<BW{
        let r:Float
        if ds==0{r=Float(x)/255}
        else if x<anchors.count{r=anchors[x]}
        else if x<64 && y<255{
            let code=y+(x*17)%(256-y)
            let s=Float((Double(code)+0.5-Double(y))/Double(255-y))
            r=x%3==0 ? s.nextDown : (x%3==1 ? s : s.nextUp)
        } else {let f=Float(next()>>8)/16777216;r=x<224 ? f : f*4-1.5}
        let g=Float((x*31+y*7+3)&255)/255,b=Float((x*11+y*47+101)&255)/255
        let alpha=Float((x*73+y*23+19)&255)/255
        a.append(SIMD4(r,g,b,alpha))
    }}
    sources.append(a)
}
var clearValues:[Double]=[-2,-0.0,0,Double.leastNonzeroMagnitude,0.1,0.5,0.625,1,2]
for k in [0,1,7,63,127,128,191,254]{let v=(Double(k)+0.5)/255;clearValues += [v.nextDown,v,v.nextUp]}
struct Texture {let t:MTLTexture;let parent:MTLBuffer?;let pitch:Int;let offset:Int;let length:Int}
struct Profile {let name:String;let rgb:MTLBlendFactor;let da:MTLBlendFactor;let sa:MTLBlendFactor;let mask:MTLColorWriteMask;let enabled:Bool;let flip:UInt32;let repeats:Int}
let profiles:[Profile]=[
    Profile(name:"replace",rgb:.zero,da:.zero,sa:.one,mask:.all,enabled:false,flip:0,repeats:1),
    Profile(name:"union",rgb:.oneMinusSourceColor,da:.zero,sa:.zero,mask:.all,enabled:true,flip:0,repeats:1),
    Profile(name:"over-alpha",rgb:.oneMinusSourceAlpha,da:.zero,sa:.zero,mask:.all,enabled:true,flip:0,repeats:1),
    Profile(name:"union-alpha-control",rgb:.oneMinusSourceColor,da:.oneMinusSourceAlpha,sa:.zero,mask:.all,enabled:true,flip:0,repeats:1),
    Profile(name:"union-red-mask",rgb:.oneMinusSourceColor,da:.zero,sa:.zero,mask:.red,enabled:true,flip:0,repeats:1),
    Profile(name:"union-no-red",rgb:.oneMinusSourceColor,da:.zero,sa:.zero,mask:[.green,.blue,.alpha],enabled:true,flip:0,repeats:1),
    Profile(name:"union-flip-alpha",rgb:.oneMinusSourceColor,da:.zero,sa:.zero,mask:.all,enabled:true,flip:1,repeats:1),
    Profile(name:"union-repeat",rgb:.oneMinusSourceColor,da:.zero,sa:.zero,mask:.all,enabled:true,flip:0,repeats:4)
]
func run()throws->[String:Any]{
    try need(CommandLine.arguments.count==3,"probe.metal outdir")
    let out=URL(fileURLWithPath:CommandLine.arguments[2]);let src=try String(contentsOfFile:CommandLine.arguments[1],encoding:.utf8)
    guard let dev=MTLCreateSystemDefaultDevice(),let queue=dev.makeCommandQueue()else{throw Failure(message:"UNAVAILABLE: device/queue")}
    var libraries:[String:MTLLibrary]=[:],renderPipelines:[String:MTLRenderPipelineState]=[:],pipelineInfo:[[String:Any]]=[]
    for mode in ["safe","default"]{
        let opt=MTLCompileOptions();if mode=="safe"{if #available(macOS 15.0,*){opt.mathMode = .safe}else{opt.fastMathEnabled=false}}
        let lib=try dev.makeLibrary(source:src,options:opt);libraries[mode]=lib
        for type in ["float","half"]{for p in profiles{
            let desc=MTLRenderPipelineDescriptor();desc.vertexFunction=lib.makeFunction(name:"triangle");desc.fragmentFunction=lib.makeFunction(name:"output_"+type)
            let c=desc.colorAttachments[0]!;c.pixelFormat = .r8Unorm;c.isBlendingEnabled=p.enabled;c.sourceRGBBlendFactor = .one;c.destinationRGBBlendFactor=p.rgb;c.rgbBlendOperation = .add
            c.sourceAlphaBlendFactor=p.sa;c.destinationAlphaBlendFactor=p.da;c.alphaBlendOperation = .add;c.writeMask=p.mask
            desc.colorAttachments[1].pixelFormat = .rgba32Float;desc.colorAttachments[1].isBlendingEnabled=false
            let key=mode+"-"+type+"-"+p.name;renderPipelines[key]=try dev.makeRenderPipelineState(descriptor:desc)
            pipelineInfo.append(["key":key,"format":c.pixelFormat.rawValue,"blend":c.isBlendingEnabled,"sourceRGB":c.sourceRGBBlendFactor.rawValue,"destinationRGB":c.destinationRGBBlendFactor.rawValue,"rgbOperation":c.rgbBlendOperation.rawValue,"sourceAlpha":c.sourceAlphaBlendFactor.rawValue,"destinationAlpha":c.destinationAlphaBlendFactor.rawValue,"alphaOperation":c.alphaBlendOperation.rawValue,"writeMask":c.writeMask.rawValue])
        }}
    }
    func cps(_ n:String)throws->MTLComputePipelineState{guard let f=libraries["safe"]!.makeFunction(name:n)else{throw Failure(message:n)};return try dev.makeComputePipelineState(function:f)}
    let readPSO=try cps("direct_read"),sampleF=try cps("sample_float"),sampleH=try cps("sample_half")
    func buf(_ d:Data)throws->MTLBuffer{try d.withUnsafeBytes{p throws -> MTLBuffer in guard let b=dev.makeBuffer(bytes:p.baseAddress!,length:p.count,options:.storageModeShared)else{throw Failure(message:"buffer")};return b}}
    func poison(_ n:Int)throws->MTLBuffer{try buf(Data(repeating:0xcd,count:n))}
    func finish(_ cb:MTLCommandBuffer)throws{cb.commit();cb.waitUntilCompleted();try need(cb.status == .completed && cb.error == nil,"GPU command \(cb.status.rawValue) \(String(describing:cb.error))")}
    func blit(_ body:(MTLBlitCommandEncoder)->Void)throws{guard let cb=queue.makeCommandBuffer(),let e=cb.makeBlitCommandEncoder()else{throw Failure(message:"blit")};body(e);e.endEncoding();try finish(cb)}
    let usage:MTLTextureUsage=[.shaderRead,.shaderWrite,.renderTarget]
    func make(_ variant:String,_ w:Int,_ h:Int,_ seed:[UInt8])throws->Texture{
        let mode:MTLStorageMode=variant.hasSuffix("private") ? .private : .shared
        let d=MTLTextureDescriptor.texture2DDescriptor(pixelFormat:.r8Unorm,width:w,height:h,mipmapped:false);d.storageMode=mode;d.usage=usage
        let pitch=((w+255)/256)*256+256,offset=4096,length=offset+pitch*h+4096
        var backing=[UInt8](repeating:0xa5,count:length)
        for y in 0..<h{for x in 0..<w{backing[offset+y*pitch+x]=seed[y*w+x]}}
        let upload=try buf(Data(backing))
        if variant.hasPrefix("buffer"){
            let opt:MTLResourceOptions=mode == .private ? .storageModePrivate : .storageModeShared
            guard let parent=dev.makeBuffer(length:length,options:opt)else{throw Failure(message:"parent")}
            if mode == .private{try blit{$0.copy(from:upload,sourceOffset:0,to:parent,destinationOffset:0,size:length)}}else{memcpy(parent.contents(),upload.contents(),length)}
            let align=dev.minimumLinearTextureAlignment(for:.r8Unorm);try need(pitch%align==0 && offset%align==0,"alignment")
            guard let t=parent.makeTexture(descriptor:d,offset:offset,bytesPerRow:pitch)else{throw Failure(message:"buffer texture nil")}
            return Texture(t:t,parent:parent,pitch:pitch,offset:offset,length:length)
        }
        guard let t=dev.makeTexture(descriptor:d)else{throw Failure(message:"texture nil")}
        if mode == .private{try blit{$0.copy(from:upload,sourceOffset:offset,sourceBytesPerRow:pitch,sourceBytesPerImage:pitch*h,sourceSize:MTLSize(width:w,height:h,depth:1),to:t,destinationSlice:0,destinationLevel:0,destinationOrigin:MTLOrigin(x:0,y:0,z:0))}}
        else{Data(seed).withUnsafeBytes{t.replace(region:MTLRegionMake2D(0,0,w,h),mipmapLevel:0,withBytes:$0.baseAddress!,bytesPerRow:w)}}
        return Texture(t:t,parent:nil,pitch:0,offset:0,length:0)
    }
    func readback(_ t:MTLTexture,_ bpp:Int)throws->Data{
        let pitch=((t.width*bpp+255)/256)*256;let b=try poison(pitch*t.height)
        try blit{$0.copy(from:t,sourceSlice:0,sourceLevel:0,sourceOrigin:MTLOrigin(x:0,y:0,z:0),sourceSize:MTLSize(width:t.width,height:t.height,depth:1),to:b,destinationOffset:0,destinationBytesPerRow:pitch,destinationBytesPerImage:pitch*t.height)}
        var result=Data();for y in 0..<t.height{result.append(Data(bytes:b.contents()+y*pitch,count:t.width*bpp))};return result
    }
    func guardCheck(_ a:Texture)throws->Int{
        guard let parent=a.parent else{return 0};let b=try poison(a.length)
        try blit{$0.copy(from:parent,sourceOffset:0,to:b,destinationOffset:0,size:a.length)}
        let q=b.contents().bindMemory(to:UInt8.self,capacity:a.length);var bad=0
        for i in 0..<a.length{let j=i-a.offset;let active=j>=0 && j<a.pitch*a.t.height && j%a.pitch<a.t.width;if !active && q[i] != 0xa5{bad += 1}}
        return bad
    }
    var hashes:[String:String]=[:],byHash:[String:String]=[:];var total=0
    func save(_ n:String,_ d:Data)throws->String{let h=sha(d);if let f=byHash[h]{return f};total += d.count;try need(total<=22*1024*1024,"artifact budget");try d.write(to:out.appendingPathComponent(n));hashes[n]=h;byHash[h]=n;return n}
    let sf=try save("small-seed-r8.bin",Data(smallSeed)),bf=try save("blend-seed-r8.bin",Data(bigSeed)),uf=try save("uv-f32x2.bin",data(uv))
    var sourceFiles:[String]=[];for (i,a) in sources.enumerated(){sourceFiles.append(try save("source\(i)-f32x4.bin",data(a)))}
    let sourceBuffers=try sources.map{try buf(data($0))};let ub=try buf(data(uv))
    var samplers:[MTLSamplerState]=[];var samplerInfo:[[String:Any]]=[]
    for linear in [false,true]{let d=MTLSamplerDescriptor();d.minFilter=linear ? .linear : .nearest;d.magFilter=d.minFilter;d.mipFilter = .notMipmapped;d.normalizedCoordinates=true;d.sAddressMode = .clampToEdge;d.tAddressMode = .clampToEdge;d.rAddressMode = .clampToEdge;d.maxAnisotropy=1;d.lodMinClamp=0;d.lodMaxClamp=Float.greatestFiniteMagnitude
        guard let s=dev.makeSamplerState(descriptor:d)else{throw Failure(message:"sampler")};samplers.append(s);samplerInfo.append(["min":d.minFilter.rawValue,"mag":d.magFilter.rawValue,"mip":d.mipFilter.rawValue,"normalized":d.normalizedCoordinates,"lodMinBits":d.lodMinClamp.bitPattern,"lodMaxBits":d.lodMaxClamp.bitPattern,"addressSTR":[d.sAddressMode.rawValue,d.tAddressMode.rawValue,d.rAddressMode.rawValue],"anisotropy":d.maxAnisotropy])}
    func direct(_ t:MTLTexture)throws->(Data,Data){
        let n=t.width*t.height,fb=try poison(n*16),hb=try poison(n*8)
        guard let cb=queue.makeCommandBuffer(),let e=cb.makeComputeCommandEncoder()else{throw Failure(message:"read encoder")}
        e.setComputePipelineState(readPSO);e.setTexture(t,index:0);e.setTexture(t,index:1);e.setBuffer(fb,offset:0,index:0);e.setBuffer(hb,offset:0,index:1)
        e.dispatchThreadgroups(MTLSize(width:(t.width+7)/8,height:(t.height+7)/8,depth:1),threadsPerThreadgroup:MTLSize(width:8,height:8,depth:1));e.endEncoding();try finish(cb)
        return (Data(bytes:fb.contents(),count:n*16),Data(bytes:hb.contents(),count:n*8))
    }
    var observations:[[String:Any]]=[],clears:[[String:Any]]=[],blends:[[String:Any]]=[],errors:[[String:Any]]=[]
    for variant in ["standalone-shared","standalone-private","buffer-shared","buffer-private"]{
        let a:Texture
        do{a=try make(variant,16,16,smallSeed)}catch{errors.append(["variant":variant,"section":"identity","error":"\(error)"]);continue}
        var obs:[String:Any]=["variant":variant,"pixelFormat":a.t.pixelFormat.rawValue,"usage":a.t.usage.rawValue,"storage":a.t.storageMode.rawValue,"bufferView":a.t.buffer != nil,"pitch":a.pitch,"offset":a.offset,"parentLength":a.length]
        obs["raw"]=try save("identity-\(variant).bin",readback(a.t,1))
        let (f,h)=try direct(a.t);obs["read_float"]=try save("read-float-\(variant).bin",f);obs["read_half"]=try save("read-half-\(variant).bin",h)
        for (ty,pso,bpp) in [("float",sampleF,16),("half",sampleH,8)]{
            let nb=try poison(uv.count*bpp),lb=try poison(uv.count*bpp);var n=UInt32(uv.count)
            guard let cb=queue.makeCommandBuffer(),let e=cb.makeComputeCommandEncoder()else{throw Failure(message:"sample encoder")}
            e.setComputePipelineState(pso);e.setTexture(a.t,index:0);e.setSamplerState(samplers[0],index:0);e.setSamplerState(samplers[1],index:1)
            e.setBuffer(ub,offset:0,index:0);e.setBuffer(nb,offset:0,index:1);e.setBuffer(lb,offset:0,index:2);e.setBytes(&n,length:4,index:3)
            e.dispatchThreadgroups(MTLSize(width:(uv.count+63)/64,height:1,depth:1),threadsPerThreadgroup:MTLSize(width:64,height:1,depth:1));e.endEncoding();try finish(cb)
            obs["sample_nearest_"+ty]=try save("sample-near-\(ty)-\(variant).bin",Data(bytes:nb.contents(),count:uv.count*bpp))
            obs["sample_linear_"+ty]=try save("sample-linear-\(ty)-\(variant).bin",Data(bytes:lb.contents(),count:uv.count*bpp))
        }
        for (i,r) in clearValues.enumerated(){
            let rp=MTLRenderPassDescriptor();rp.colorAttachments[0].texture=a.t;rp.colorAttachments[0].loadAction = .clear;rp.colorAttachments[0].storeAction = .store
            rp.colorAttachments[0].clearColor=MTLClearColor(red:r,green:0.8125,blue:0.375,alpha:i%2==0 ? 0.125 : 0.9375)
            guard let cb=queue.makeCommandBuffer(),let e=cb.makeRenderCommandEncoder(descriptor:rp)else{throw Failure(message:"clear encoder")};e.endEncoding();try finish(cb)
            let raw=try readback(a.t,1);let (rf,rh)=try direct(a.t)
            clears.append(["variant":variant,"index":i,"red_f64_bits":String(format:"%016llx",r.bitPattern),"raw":try save("clear\(i)-\(variant).bin",raw),"read_float":try save("clear\(i)-readf-\(variant).bin",rf),"read_half":try save("clear\(i)-readh-\(variant).bin",rh)])
        }
        obs["padding_bad_after_clear"]=try guardCheck(a);observations.append(obs)
        for mode in ["safe","default"]{for ds in 0..<sources.count{for ty in ["float","half"]{for p in profiles{
            let tex=try make(variant,BW,BH,bigSeed)
            let dd=MTLTextureDescriptor.texture2DDescriptor(pixelFormat:.rgba32Float,width:BW,height:BH,mipmapped:false);dd.storageMode = .private;dd.usage=[.renderTarget,.shaderRead]
            guard let emitted=dev.makeTexture(descriptor:dd)else{throw Failure(message:"emitted texture")}
            var pp=SIMD2<UInt32>(UInt32(BW),p.flip)
            for iteration in 1...p.repeats{
                let rp=MTLRenderPassDescriptor();rp.colorAttachments[0].texture=tex.t;rp.colorAttachments[0].loadAction = .load;rp.colorAttachments[0].storeAction = .store
                rp.colorAttachments[1].texture=emitted;rp.colorAttachments[1].loadAction = .dontCare;rp.colorAttachments[1].storeAction = .store
                guard let cb=queue.makeCommandBuffer(),let e=cb.makeRenderCommandEncoder(descriptor:rp)else{throw Failure(message:"render encoder")}
                e.setRenderPipelineState(renderPipelines[mode+"-"+ty+"-"+p.name]!);e.setFragmentBuffer(sourceBuffers[ds],offset:0,index:0);e.setFragmentBytes(&pp,length:8,index:1)
                e.drawPrimitives(type:.triangle,vertexStart:0,vertexCount:3);e.endEncoding();try finish(cb)
                let tag="b\(blends.count)";let raw=try readback(tex.t,1);let em=try readback(emitted,16)
                blends.append(["variant":variant,"math":mode,"type":ty,"source":ds,"profile":p.name,"iteration":iteration,"flipAlpha":p.flip,"raw":try save(tag+"-r8.bin",raw),"emitted":try save(tag+"-emitted-f32x4.bin",em),"padding_bad":try guardCheck(tex)])
            }
        }}}}
    }
    var device:[String:Any]=["name":dev.name,"registryID":dev.registryID,"hasUnifiedMemory":dev.hasUnifiedMemory,"mac2":dev.supportsFamily(.mac2),"apple7":dev.supportsFamily(.apple7),"linear_alignment_r8":dev.minimumLinearTextureAlignment(for:.r8Unorm)]
    if #available(macOS 14.0,*){device["architecture"]=dev.architecture.name}
    return ["status":errors.isEmpty ? "OBSERVATIONS_COMPLETE" : "PARTIAL_VARIANT_FAILURE","device":device,"os":ProcessInfo.processInfo.operatingSystemVersionString,"identities":observations,"clears":clears,"blends":blends,"pipelines":pipelineInfo,"samplers":samplerInfo,"errors":errors,"files_sha256":hashes,"data_bytes":total,"small_shape":[16,16],"blend_shape":[BW,BH],"small_seed":sf,"blend_seed":bf,"uv":uf,"uv_count":uv.count,"coordinate_sets":coordSets,"source_files":sourceFiles,
            "layout":"raw R8 files are tightly packed row-major bytes; read/sample-float and emitted are RGBA u32 binary32 words; half read/sample outputs are RGBA u16 binary16 words; source arrays are float4; uv is float2" ]
}
let out=CommandLine.arguments.count>2 ? CommandLine.arguments[2] : ".";var result:[String:Any];var code:Int32=0
do{result=try run()}catch{result=["status":"FAILED_NOT_PASS","error":"\(error)"];code=3}
let json=try! JSONSerialization.data(withJSONObject:result,options:[.prettyPrinted,.sortedKeys]);try! json.write(to:URL(fileURLWithPath:out).appendingPathComponent("summary.json"));FileHandle.standardOutput.write(json.prefix(3000));exit(code)
