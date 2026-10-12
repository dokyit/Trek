import SwiftUI

/// An interface mockup: a tree of semantic UI parts drawn in Trek's own controls, optionally inside
/// a device's chrome. Images are placeholders by design: nothing is ever loaded.
struct VizMockup: View {
    var nodes: [MockNode]
    var look: VizLook
    var enter: Double

    var body: some View {
        let framed = nodes.contains { $0.kind == .frame }
        let content = VStack(alignment: framed ? .center : .leading, spacing: 12) {
            ForEach(nodes.indices, id: \.self) { i in MockPart(node: nodes[i], look: look, enter: enter) }
        }
        .frame(maxWidth: .infinity, alignment: framed ? .center : .leading)
        if framed {
            // Devices stand on the inset surface, like artboards.
            content.vizInset(look, padding: 12)
        } else {
            content
                .padding(12)
                .background(Trek.background, in: RoundedRectangle(cornerRadius: 11, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 11, style: .continuous).strokeBorder(look.hairline, lineWidth: 1))
        }
    }
}

/// Up to two initials from a name.
func vizInitials(_ name: String) -> String {
    String(name.split(whereSeparator: \.isWhitespace).compactMap { $0.first { $0.isLetter || $0.isNumber } }.prefix(2)).uppercased()
}

private struct MockPart: View {
    var node: MockNode
    var look: VizLook
    var enter: Double

    private var text: String { node.text ?? "" }
    private var color: Color { VizLook.tone(node.tone) }

    var body: some View {
        // Controls keep their own width in a column instead of stretching across it.
        switch node.kind {
        case .button, .badge, .avatar: part.frame(maxWidth: .infinity, alignment: .leading)
        default: part
        }
    }

    private func children(spacing: CGFloat = 8) -> some View {
        ForEach(node.children.indices, id: \.self) { i in MockPart(node: node.children[i], look: look, enter: enter) }
    }

    @ViewBuilder
    private var part: some View {
        switch node.kind {
        case .frame:
            MockChrome(node: node, look: look, enter: enter)
        case .row:
            VizWrap(spacing: 8, lineSpacing: 8) { children() }
                .frame(maxWidth: .infinity, alignment: .leading)
        case .column:
            VStack(alignment: .leading, spacing: 8) { children() }.frame(maxWidth: .infinity, alignment: .leading)
        case .card:
            VStack(alignment: .leading, spacing: 8) {
                if node.text != nil { Text(text).font(.system(size: 13.5, weight: .medium)).foregroundStyle(look.fg) }
                children()
            }
            .padding(12)
            .frame(minWidth: 150, maxWidth: .infinity, alignment: .leading)
            .background(look.fg.opacity(look.dark ? 0.03 : 0.015), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(look.hairline, lineWidth: 1))
        case .list:
            VStack(alignment: .leading, spacing: 6) {
                if node.text != nil { Text(text).font(.system(size: 12, weight: .medium)).foregroundStyle(look.muted).padding(.horizontal, 4) }
                VStack(spacing: 0) {
                    ForEach(node.children.indices, id: \.self) { i in
                        let child = node.children[i]
                        HStack(spacing: 8) {
                            MockPart(node: child, look: look, enter: enter).frame(maxWidth: .infinity, alignment: .leading)
                            if child.kind == .text {
                                Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)).foregroundStyle(look.muted.opacity(0.7))
                            }
                        }
                        .padding(.horizontal, 12)
                        .padding(.vertical, 10)
                        .overlay(alignment: .top) { if i > 0 { Rectangle().fill(look.grid).frame(height: 1) } }
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 9, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(look.hairline, lineWidth: 1))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .nav:
            HStack(spacing: 12) {
                if node.text != nil { Text(text).font(.system(size: 14, weight: .semibold)).foregroundStyle(look.fg).lineLimit(1) }
                Spacer(minLength: 0)
                ForEach(node.children.indices, id: \.self) { i in
                    let child = node.children[i]
                    if child.kind == .text {
                        Text(child.text ?? "").font(.system(size: 12.5)).foregroundStyle(look.muted).lineLimit(1)
                    } else {
                        MockPart(node: child, look: look, enter: enter).fixedSize()
                    }
                }
            }
            .padding(.horizontal, 12)
            .frame(height: 44)
            .overlay(alignment: .bottom) { Rectangle().fill(look.hairline).frame(height: 1) }
        case .tabs:
            let active = node.value ?? node.children.first?.text
            HStack(alignment: .bottom, spacing: 18) {
                ForEach(node.children.indices, id: \.self) { i in
                    let label = node.children[i].text ?? ""
                    let on = label == active
                    VStack(spacing: 7) {
                        Text(label).font(.system(size: 13, weight: on ? .medium : .regular)).foregroundStyle(on ? look.fg : look.muted).lineLimit(1)
                        UnevenRoundedRectangle(topLeadingRadius: 2, topTrailingRadius: 2).fill(on ? color : .clear).frame(height: 2)
                    }
                    .fixedSize()
                    .padding(.top, 4)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(alignment: .bottom) { Rectangle().fill(look.hairline).frame(height: 1) }
        case .text:
            Text(text).font(.system(size: 13)).foregroundStyle(look.fg.opacity(0.88)).lineSpacing(2).fixedSize(horizontal: false, vertical: true)
        case .metric:
            VStack(alignment: .leading, spacing: 2) {
                Text(text).font(.system(size: 12)).foregroundStyle(look.muted)
                Text(node.value ?? "").font(.system(size: 19, weight: .semibold)).foregroundStyle(node.tone != nil ? color : look.fg)
            }
            .frame(minWidth: 100, alignment: .leading)
        case .button:
            let primary = node.tone == nil || node.tone == .accent
            let neutral = node.tone == .neutral
            Text(text)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(primary ? .white : neutral ? look.fg : color)
                .padding(.horizontal, 13)
                .frame(height: 32)
                .background {
                    let shape = RoundedRectangle(cornerRadius: 8, style: .continuous)
                    if primary {
                        shape.fill(color).shadow(color: color.opacity(0.3), radius: 1.5, y: 1)
                    } else if neutral {
                        shape.strokeBorder(look.fg.opacity(0.16), lineWidth: 1)
                    } else {
                        shape.fill(color.opacity(0.12))
                    }
                }
        case .badge:
            HStack(spacing: 5) {
                Circle().fill(color).frame(width: 5, height: 5)
                Text(text).font(.system(size: 11.5, weight: .medium))
            }
            .foregroundStyle(color)
            .padding(.horizontal, 8)
            .frame(height: 21)
            .background(color.opacity(0.12), in: Capsule())
        case .divider:
            Rectangle().fill(look.hairline).frame(height: 1)
        case .input:
            let typed = node.value
            VStack(alignment: .leading, spacing: 4) {
                if typed != nil { Text(text).font(.system(size: 12)).foregroundStyle(look.muted) }
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").font(.system(size: 12)).foregroundStyle(look.muted)
                    Text(typed ?? text).font(.system(size: 13)).foregroundStyle(typed != nil ? look.fg : look.muted.opacity(0.8)).lineLimit(1)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 10)
                .frame(height: 34)
                .background(Trek.background, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(typed != nil ? color.opacity(0.6) : look.fg.opacity(0.14), lineWidth: 1))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .toggle:
            let on = node.value == "on"
            HStack(spacing: 12) {
                Text(text).font(.system(size: 13.5)).foregroundStyle(look.fg)
                Spacer(minLength: 0)
                Capsule()
                    .fill(on ? color : look.fg.opacity(0.16))
                    .frame(width: 36, height: 21)
                    .overlay(alignment: on ? .trailing : .leading) {
                        Circle().fill(.white).frame(width: 17, height: 17).shadow(color: .black.opacity(0.25), radius: 1, y: 1).padding(2)
                    }
            }
            .frame(maxWidth: .infinity)
        case .progress:
            let value = node.value ?? ""
            let frac = (VizSchema.progressFraction(value) ?? 0) * enter
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text(text).font(.system(size: 13)).foregroundStyle(look.fg)
                    Spacer(minLength: 8)
                    Text(value.hasSuffix("%") ? value : "\(value)%").font(.system(size: 12, design: .monospaced)).foregroundStyle(look.muted)
                }
                Capsule().fill(look.fg.opacity(0.08)).frame(height: 6)
                    .overlay(alignment: .leading) {
                        GeometryReader { g in Capsule().fill(color).frame(width: g.size.width * frac) }
                    }
            }
            .frame(maxWidth: .infinity)
        case .avatar:
            let tint = node.tone != nil ? color : VizLook.series(text.utf8.reduce(0) { ($0 &* 31 &+ Int($1)) & 0xFFFF })
            Text(vizInitials(text))
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(look.fg)
                .frame(width: 30, height: 30)
                .background(Circle().fill(look.gap).overlay(Circle().fill(tint.opacity(0.28))))
                .overlay(Circle().strokeBorder(tint.opacity(0.45), lineWidth: 1))
        case .image:
            let (w, h): (CGFloat?, CGFloat) = switch node.value {
            case "square": (140, 140)
            case "tall": (130, 190)
            default: (nil, 132)
            }
            VStack(spacing: 6) {
                Image(systemName: "photo").font(.system(size: 20)).foregroundStyle(look.muted.opacity(0.7))
                if node.text != nil { Text(text).font(.system(size: 12)).foregroundStyle(look.muted).lineLimit(1).padding(.horizontal, 8) }
            }
            .frame(width: w, height: h)
            .frame(maxWidth: w == nil ? .infinity : nil)
            .background(LinearGradient(colors: [look.fg.opacity(0.07), look.fg.opacity(0.025)], startPoint: .topLeading, endPoint: .bottomTrailing),
                        in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(look.grid, lineWidth: 1))
        }
    }
}

/// A frame's chrome: a phone or tablet with its status bar, or a browser or window with its
/// traffic lights.
private struct MockChrome: View {
    var node: MockNode
    var look: VizLook
    var enter: Double

    var body: some View {
        let device = node.device ?? .window
        let content = VStack(alignment: .leading, spacing: 12) {
            ForEach(node.children.indices, id: \.self) { i in MockPart(node: node.children[i], look: look, enter: enter) }
        }
        Group {
            switch device {
            case .phone, .tablet:
                let phone = device == .phone
                VStack(spacing: 0) {
                    HStack {
                        Text("9:41").font(.system(size: 12.5, weight: .semibold))
                        Spacer()
                        HStack(spacing: 4) {
                            Image(systemName: "cellularbars")
                            Image(systemName: "wifi")
                            Image(systemName: "battery.100percent")
                        }
                        .font(.system(size: 11))
                    }
                    .foregroundStyle(look.fg)
                    .padding(.horizontal, phone ? 24 : 18)
                    .frame(height: phone ? 36 : 28)
                    VStack(alignment: .leading, spacing: 12) {
                        if let title = node.text { Text(title).font(.system(size: 21, weight: .semibold)).foregroundStyle(look.fg) }
                        content
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 8)
                    .padding(.bottom, 12)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    if phone {
                        Capsule().fill(look.fg.opacity(0.35)).frame(width: 108, height: 4).frame(height: 20)
                    }
                }
                .frame(minHeight: phone ? 440 : 300)
                .background(Trek.background)
                .overlay(alignment: .top) {
                    if phone { Capsule().fill(.black).frame(width: 86, height: 24).padding(.top, 10) }
                }
                .clipShape(RoundedRectangle(cornerRadius: phone ? 30 : 14, style: .continuous))
                .padding(phone ? 7 : 10)
                .background(look.dark ? AnyShapeStyle(look.fg.opacity(0.07)) : AnyShapeStyle(Color.black.opacity(0.82)),
                            in: RoundedRectangle(cornerRadius: phone ? 38 : 24, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: phone ? 38 : 24, style: .continuous).strokeBorder(look.fg.opacity(look.dark ? 0.16 : 0.14), lineWidth: 1))
                .frame(maxWidth: phone ? 296 : 560)
            case .browser, .window:
                let browser = device == .browser
                VStack(spacing: 0) {
                    HStack(spacing: 12) {
                        HStack(spacing: 6) {
                            ForEach([0xFF5F57, 0xFEBC2E, 0x28C840] as [UInt32], id: \.self) { c in
                                Circle().fill(Color(hex: c).opacity(look.dark ? 0.85 : 1)).frame(width: 10, height: 10)
                            }
                        }
                        if browser {
                            Image(systemName: "chevron.left").font(.system(size: 12, weight: .medium)).foregroundStyle(look.muted.opacity(0.7))
                            HStack(spacing: 6) {
                                Image(systemName: "lock.fill").font(.system(size: 9))
                                Text(node.text ?? "app").font(.system(size: 11.5)).lineLimit(1)
                            }
                            .foregroundStyle(look.muted)
                            .padding(.horizontal, 10)
                            .frame(maxWidth: .infinity)
                            .frame(height: 24)
                            .background(look.fg.opacity(0.06), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                        } else {
                            Text(node.text ?? "").font(.system(size: 12.5, weight: .medium)).foregroundStyle(look.fg.opacity(0.8)).lineLimit(1)
                                .frame(maxWidth: .infinity)
                        }
                        Color.clear.frame(width: browser ? 8 : 46, height: 1)
                    }
                    .padding(.horizontal, 12)
                    .frame(height: browser ? 40 : 34)
                    .background(look.fg.opacity(look.dark ? 0.04 : 0.03))
                    .overlay(alignment: .bottom) { Rectangle().fill(look.hairline).frame(height: 1) }
                    content.padding(14).frame(maxWidth: .infinity, alignment: .leading)
                }
                .background(Trek.background)
                .clipShape(RoundedRectangle(cornerRadius: 11, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 11, style: .continuous).strokeBorder(look.fg.opacity(look.dark ? 0.14 : 0.12), lineWidth: 1))
            }
        }
        .shadow(color: .black.opacity(look.dark ? 0.5 : 0.12), radius: 16, y: 12)
        .frame(maxWidth: .infinity)
    }
}
