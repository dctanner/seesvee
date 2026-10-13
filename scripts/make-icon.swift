// Renders assets/Seesvee.icns: a rounded tile with a small spreadsheet grid.
// Usage: swift scripts/make-icon.swift
import AppKit

let size: CGFloat = 1024
let image = NSImage(size: NSSize(width: size, height: size))
image.lockFocus()
let ctx = NSGraphicsContext.current!.cgContext

// macOS icon grid: 824pt tile centered in 1024 canvas.
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let tilePath = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: NSColor(white: 0, alpha: 0.28).cgColor)
ctx.addPath(tilePath)
ctx.setFillColor(NSColor.white.cgColor)
ctx.fillPath()
ctx.restoreGState()

ctx.saveGState()
ctx.addPath(tilePath)
ctx.clip()
let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(),
    colors: [NSColor(red: 1, green: 1, blue: 1, alpha: 1).cgColor,
             NSColor(red: 0.93, green: 0.95, blue: 0.98, alpha: 1).cgColor] as CFArray,
    locations: [0, 1])!
ctx.drawLinearGradient(gradient, start: CGPoint(x: 0, y: tile.maxY), end: CGPoint(x: 0, y: tile.minY), options: [])

// Grid
let accent = NSColor(red: 0.04, green: 0.42, blue: 1.0, alpha: 1)
let grid = tile.insetBy(dx: 150, dy: 170)
let rows = 5, cols = 3
let rowH = grid.height / CGFloat(rows)
let colW = grid.width / CGFloat(cols)
// Header row
let header = CGRect(x: grid.minX, y: grid.maxY - rowH, width: grid.width, height: rowH)
let headerPath = CGPath(roundedRect: header, cornerWidth: 26, cornerHeight: 26, transform: nil)
ctx.addPath(headerPath)
ctx.setFillColor(accent.cgColor)
ctx.fillPath()
// Body cells
for r in 0..<(rows - 1) {
    for c in 0..<cols {
        let cell = CGRect(x: grid.minX + CGFloat(c) * colW + 9,
                          y: grid.minY + CGFloat(r) * rowH + 9,
                          width: colW - 18, height: rowH - 18)
        let p = CGPath(roundedRect: cell, cornerWidth: 16, cornerHeight: 16, transform: nil)
        ctx.addPath(p)
        let highlighted = (r == 2 && c == 1)
        ctx.setFillColor(highlighted ? accent.withAlphaComponent(0.22).cgColor
                                     : NSColor(red: 0.84, green: 0.87, blue: 0.92, alpha: 1).cgColor)
        ctx.fillPath()
        if highlighted {
            ctx.addPath(p)
            ctx.setStrokeColor(accent.cgColor)
            ctx.setLineWidth(10)
            ctx.strokePath()
        }
    }
}
ctx.restoreGState()
image.unlockFocus()

let rep = NSBitmapImageRep(data: image.tiffRepresentation!)!
let png = rep.representation(using: .png, properties: [:])!
let out = URL(fileURLWithPath: CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon_1024.png")
try! png.write(to: out)
