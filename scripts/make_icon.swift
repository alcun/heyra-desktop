// Draws Heyra's app icon (assets/AppIcon.icns): the three-bar mark on graphite.
// Run: swift scripts/make_icon.swift
import AppKit

func png(_ size: Int) -> Data {
    let s = CGFloat(size)
    let image = NSImage(size: NSSize(width: s, height: s))
    image.lockFocus()
    let inset = s * 0.1
    let tile = NSRect(x: inset, y: inset, width: s - 2 * inset, height: s - 2 * inset)
    NSColor(red: 0x23/255, green: 0x25/255, blue: 0x2d/255, alpha: 1).setFill()
    NSBezierPath(roundedRect: tile, xRadius: tile.width * 0.225, yRadius: tile.width * 0.225).fill()
    // bars: (x centre, half height, colour)
    let cream = NSColor(red: 0xed/255, green: 0xe0/255, blue: 0xc4/255, alpha: 1)
    let gold = NSColor(red: 0xd9/255, green: 0xa8/255, blue: 0x6a/255, alpha: 1)
    let bars: [(CGFloat, CGFloat, NSColor)] = [(0.36, 0.10, cream), (0.50, 0.20, gold), (0.64, 0.13, cream)]
    let w = s * 0.075
    for (x, hh, colour) in bars {
        colour.setFill()
        let r = NSRect(x: s * x - w / 2, y: s * (0.5 - hh), width: w, height: s * hh * 2)
        NSBezierPath(roundedRect: r, xRadius: w / 2, yRadius: w / 2).fill()
    }
    image.unlockFocus()
    let rep = NSBitmapImageRep(data: image.tiffRepresentation!)!
    return rep.representation(using: .png, properties: [:])!
}

let set = URL(fileURLWithPath: "assets/AppIcon.iconset")
try? FileManager.default.createDirectory(at: set, withIntermediateDirectories: true)
for base in [16, 32, 128, 256, 512] {
    try! png(base).write(to: set.appendingPathComponent("icon_\(base)x\(base).png"))
    try! png(base * 2).write(to: set.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
let task = Process()
task.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
task.arguments = ["-c", "icns", set.path, "-o", "assets/AppIcon.icns"]
try! task.run(); task.waitUntilExit()
try? FileManager.default.removeItem(at: set)
print("wrote assets/AppIcon.icns")
