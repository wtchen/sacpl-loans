// Generates the SacPL Loans app icon master (1024x1024) and the menu-bar
// (tray) template icon. Drawn procedurally so the artwork stays editable.
//
// Usage:  swift tools/make_icons.swift src-tauri/icons
// Afterward regenerate all platform icons from the master:
//   npx tauri icon src-tauri/icons/app-icon-1024.png
//
// Design: an open book on an indigo "library" gradient squircle with a coral
// bookmark ribbon. Flat, cleanly layered artwork reads well inside the macOS
// 26 "Liquid Glass" icon treatment (which adds the glass/depth at render
// time). The tray icon is the same book as a pure-alpha template silhouette.

import AppKit
import Foundation

// MARK: - PNG rendering (all drawing happens in a 1024x1024 logical space)

func renderPNG(px: Int, to path: String, inset: CGFloat = 0, draw: (CGContext) -> Void) throws {
    let ctx = CGContext(
        data: nil, width: px, height: px,
        bitsPerComponent: 8, bytesPerRow: 0,
        space: CGColorSpaceCreateDeviceRGB(),
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    )!
    let scale = CGFloat(px) * (1 - 2 * inset) / 1024.0
    ctx.translateBy(x: CGFloat(px) * inset, y: CGFloat(px) * inset)
    ctx.scaleBy(x: scale, y: scale)
    NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
    draw(ctx)
    NSGraphicsContext.current = nil
    let cg = ctx.makeImage()!
    let rep = NSBitmapImageRep(cgImage: cg)
    let data = rep.representation(using: .png, properties: [:])!
    try data.write(to: URL(fileURLWithPath: path), options: .atomic)
}

// MARK: - Book glyph

/// Half of the open book. sign = +1 right page, -1 left page.
/// Coordinates live in the 1024 logical space; the glyph spans roughly
/// x 250..774, y 364..680 (before translation).
func bookHalfPath(_ sign: CGFloat) -> NSBezierPath {
    let cx: CGFloat = 512
    func x(_ dx: CGFloat) -> CGFloat { cx + sign * dx }
    let p = NSBezierPath()
    p.move(to: NSPoint(x: x(0), y: 602)) // spine top
    p.curve(to: NSPoint(x: x(214), y: 648), // top edge waves up toward the outer corner
            controlPoint1: NSPoint(x: x(150), y: 624),
            controlPoint2: NSPoint(x: x(228), y: 680))
    p.line(to: NSPoint(x: x(248), y: 648))
    p.curve(to: NSPoint(x: x(262), y: 634), // rounded outer top corner (~14pt)
            controlPoint1: NSPoint(x: x(258), y: 648),
            controlPoint2: NSPoint(x: x(262), y: 642))
    p.line(to: NSPoint(x: x(262), y: 432)) // outer edge
    p.curve(to: NSPoint(x: x(248), y: 418), // rounded outer bottom corner
            controlPoint1: NSPoint(x: x(262), y: 424),
            controlPoint2: NSPoint(x: x(258), y: 418))
    p.line(to: NSPoint(x: x(214), y: 418))
    p.curve(to: NSPoint(x: x(0), y: 374), // bottom edge dips down to the spine
            controlPoint1: NSPoint(x: x(198), y: 402),
            controlPoint2: NSPoint(x: x(118), y: 362))
    p.close() // straight back up the spine
    return p
}

/// Bookmark ribbon hanging from the top of the right page.
func ribbonPath() -> NSBezierPath {
    let p = NSBezierPath()
    p.move(to: NSPoint(x: 592, y: 672))
    p.line(to: NSPoint(x: 626, y: 672))
    p.line(to: NSPoint(x: 626, y: 540))
    p.line(to: NSPoint(x: 609, y: 520)) // V notch
    p.line(to: NSPoint(x: 592, y: 540))
    p.close()
    return p
}

// MARK: - Artwork

func drawBook(ctx: CGContext, fill: NSColor, crease: Bool) {
    ctx.saveGState()
    ctx.translateBy(x: 0, y: -12) // optical centering on the squircle
    fill.setFill()
    bookHalfPath(-1).fill()
    bookHalfPath(1).fill()
    if crease {
        // Dark translucent crease down the spine (visible on the white book).
        ctx.setStrokeColor(NSColor(white: 0.12, alpha: 0.22).cgColor)
        ctx.setLineWidth(12)
        ctx.setLineCap(.round)
        ctx.move(to: CGPoint(x: 512, y: 596))
        ctx.addLine(to: CGPoint(x: 512, y: 382))
        ctx.strokePath()
    }
    ctx.restoreGState()
}

let outDir = CommandLine.arguments.count > 1
    ? CommandLine.arguments[1]
    : FileManager.default.currentDirectoryPath

// --- App icon master (1024) ---
try renderPNG(px: 1024, to: outDir + "/app-icon-1024.png") { ctx in
    // Squircle-ish background (matches the macOS icon canvas: 824 centered).
    let rect = NSRect(x: 100, y: 100, width: 824, height: 824)
    let bg = NSBezierPath(roundedRect: rect, xRadius: 186, yRadius: 186)
    ctx.saveGState()
    ctx.addPath(bg.cgPath)
    ctx.clip()
    // Indigo "library" gradient, lighter at the top like a lit reading room.
    let top = NSColor(calibratedRed: 0.38, green: 0.47, blue: 0.89, alpha: 1).cgColor
    let bottom = NSColor(calibratedRed: 0.15, green: 0.19, blue: 0.52, alpha: 1).cgColor
    let grad = CGGradient(colorsSpace: nil, colors: [top, bottom] as CFArray, locations: [0, 1])!
    ctx.drawLinearGradient(grad, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])

    drawBook(ctx: ctx, fill: NSColor(white: 1, alpha: 0.98), crease: true)

    // Coral bookmark ribbon with its own soft shadow to separate from the page.
    ctx.saveGState()
    ctx.translateBy(x: 0, y: -12)
    ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 16, color: NSColor(white: 0, alpha: 0.25).cgColor)
    NSColor(calibratedRed: 0.96, green: 0.36, blue: 0.30, alpha: 1).setFill()
    ribbonPath().fill()
    ctx.restoreGState()

    ctx.restoreGState()
}

// --- Menu bar template icon (pure alpha silhouette; macOS tints it) ---
try renderPNG(px: 88, to: outDir + "/tray-icon.png", inset: 0.02) { ctx in
    // Scale the glyph up around its center so it fills ~2/3 of the frame,
    // matching the optical size of neighboring menu bar icons.
    ctx.saveGState()
    ctx.translateBy(x: 512, y: 512)
    ctx.scaleBy(x: 1.28, y: 1.28)
    ctx.translateBy(x: -512, y: -512)
    drawBook(ctx: ctx, fill: .black, crease: false)
    // Carve the spine gap so the silhouette reads as two pages.
    ctx.setBlendMode(.clear)
    ctx.setLineWidth(18)
    ctx.setLineCap(.round)
    ctx.translateBy(x: 0, y: -12)
    ctx.move(to: CGPoint(x: 512, y: 596))
    ctx.addLine(to: CGPoint(x: 512, y: 382))
    ctx.strokePath()
    ctx.restoreGState()
}

print("wrote app-icon-1024.png and tray-icon.png to \(outDir)")
