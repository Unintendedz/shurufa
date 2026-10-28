#!/usr/bin/env xcrun swift
import AppKit
import Foundation

// 从已提交的 SVG 路径母稿渲染，不依赖本机字体版本，也不解析或猜测路径语法。
let source = URL(fileURLWithPath: CommandLine.arguments[1])
let output = URL(fileURLWithPath: CommandLine.arguments[2])
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
for suffix in ["", "Chinese", "Shuangpin", "English", "Wubi", "Cantonese", "Zhuyin", "Japanese", "Korean", "Vietnamese", "Tibetan", "Stroke"] {
    let name = "MSIMEClientInputMethodMenuIcon" + suffix
    guard let image = NSImage(contentsOf: source.appendingPathComponent(name + ".svg")) else {
        fatalError("无法读取已提交的 SVG 母稿：\(name)")
    }
    var pages = [String]()
    for pixels in [16, 32] {
        let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        let context = NSGraphicsContext(bitmapImageRep: bitmap)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        context.cgContext.clear(CGRect(x: 0, y: 0, width: pixels, height: pixels))
        image.draw(in: NSRect(x: 0, y: 0, width: pixels, height: pixels), from: .zero,
            operation: .sourceOver, fraction: 1, respectFlipped: false, hints: nil)
        NSGraphicsContext.restoreGraphicsState()
        let png = output.appendingPathComponent(name + (pixels == 16 ? "" : "@2x") + ".png")
        try bitmap.representation(using: .png, properties: [:])!.write(to: png)
        pages.append(png.path)
    }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/tiffutil")
    process.arguments = ["-cathidpicheck"] + pages + ["-out", output.appendingPathComponent(name + ".tiff").path]
    try process.run(); process.waitUntilExit()
    guard process.terminationStatus == 0 else { fatalError("TIFF 生成失败") }
}
