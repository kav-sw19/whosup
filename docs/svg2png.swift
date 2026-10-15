import AppKit
let args = CommandLine.arguments
guard args.count == 4, let img = NSImage(contentsOfFile: args[1]), let scale = Double(args[3]) else {
    FileHandle.standardError.write("usage: svg2png in.svg out.png scale\n".data(using: .utf8)!); exit(1)
}
let size = img.size
let w = Int(size.width * scale), h = Int(size.height * scale)
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8, samplesPerPixel: 4,
                           hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
rep.size = size
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
img.draw(in: NSRect(origin: .zero, size: size))
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: args[2]))
print("\(w)x\(h)")
