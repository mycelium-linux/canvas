// Deterministic first-party vector artwork, rasterized with Apple's native API.
// Generated PNGs/catalog metadata are build inputs, not downloaded assets.
import AppKit
import Foundation

let root = URL(fileURLWithPath:CommandLine.arguments[1],isDirectory:true)
let files = FileManager.default
func metadata(_ path: String,_ value: [String:Any]) throws {
    let directory = root.appendingPathComponent(path,isDirectory:true)
    try files.createDirectory(at:directory,withIntermediateDirectories:true)
    let data = try JSONSerialization.data(withJSONObject:value,options:[.prettyPrinted,.sortedKeys])
    try data.write(to:directory.appendingPathComponent("Contents.json"),options:.atomic)
}
func png(_ path: String,width: Int,height: Int,foreground: Bool) throws {
    let image = NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:width,pixelsHigh:height,
        bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,
        bytesPerRow:width * 4,bitsPerPixel:32)!
    let graphics = NSGraphicsContext(bitmapImageRep:image)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = graphics
    let context = graphics.cgContext
    if foreground {
        let edge = Double(height) * 0.58
        let box = CGRect(x:(Double(width)-edge)/2,y:(Double(height)-edge)/2,width:edge,height:edge)
        context.setStrokeColor(NSColor(srgbRed:0.71,green:0.91,blue:0.75,alpha:1).cgColor)
        context.setLineWidth(edge * 0.06)
        context.addPath(CGPath(roundedRect:box,cornerWidth:edge * 0.24,cornerHeight:edge * 0.24,transform:nil))
        context.strokePath()
        context.setLineCap(.round)
        context.addArc(center:CGPoint(x:box.midX,y:box.midY),radius:edge * 0.23,
            startAngle:Double.pi * 0.24,endAngle:Double.pi * 1.76,clockwise:false)
        context.strokePath()
    } else {
        context.setFillColor(NSColor(srgbRed:0.035,green:0.075,blue:0.055,alpha:1).cgColor)
        context.fill(CGRect(x:0,y:0,width:width,height:height))
    }
    NSGraphicsContext.restoreGraphicsState()
    try image.representation(using:.png,properties:[:])!.write(to:root.appendingPathComponent(path),options:.atomic)
}
let info: [String:Any] = ["author":"xcode","version":1]
if !files.fileExists(atPath:root.appendingPathComponent("Contents.json").path) {
    try metadata("",["info":info])
}
try metadata("AccentColor.colorset",["info":info,"colors":[["idiom":"universal","color":["color-space":"srgb","components":["red":"0.710","green":"0.910","blue":"0.750","alpha":"1.000"]]]]])
try metadata("AppIcon.brandassets",["info":info,"assets":[
    ["idiom":"tv","role":"primary-app-icon","size":"400x240","filename":"Small.imagestack"],
    ["idiom":"tv","role":"primary-app-icon","size":"1280x768","filename":"Large.imagestack"],
    ["idiom":"tv","role":"top-shelf-image","size":"1920x720","filename":"TopShelf.imageset"]]])
for (name,width,height) in [("Small",400,240),("Large",1280,768)] {
    let base = "AppIcon.brandassets/\(name).imagestack"
    try metadata(base,["info":info,"layers":[["filename":"Front.imagestacklayer"],["filename":"Back.imagestacklayer"]]])
    for (layer,foreground) in [("Front",true),("Back",false)] {
        let folder = "\(base)/\(layer).imagestacklayer"
        try metadata(folder,["info":info])
        var images: [[String:String]] = [["idiom":"tv","scale":"1x","filename":"art.png"]]
        if name == "Small" {
            images.append(["idiom":"tv","scale":"2x","filename":"art@2x.png"])
        }
        try metadata(folder + "/Content.imageset",["info":info,"images":images])
        try png(folder + "/Content.imageset/art.png",width:width,height:height,foreground:foreground)
        if name == "Small" {
            try png(folder + "/Content.imageset/art@2x.png",width:width * 2,height:height * 2,foreground:foreground)
        }
    }
}
let top = "AppIcon.brandassets/TopShelf.imageset"
try metadata(top,["info":info,"images":[["idiom":"tv","scale":"1x","filename":"art.png"],
    ["idiom":"tv","scale":"2x","filename":"art@2x.png"]]])
try png(top + "/art.png",width:1920,height:720,foreground:false)
try png(top + "/art@2x.png",width:3840,height:1440,foreground:false)
