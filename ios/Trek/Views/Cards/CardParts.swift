import SwiftUI
import UIKit

// What the live cards (`PrCard`, `RunCard`, `ReleaseCard`) and the composer's chip are made of:
// the Mac card's verdict colours and neutrals, its tile, badge, branch pill, state marks and
// buttons, at the phone's sizes (Figma: "Phone · cards"). Colour only for verdicts; the rest is
// the page's own ink.

// MARK: Colour

/// A verdict colour, as the Mac's cards have it (`pr_card::hues`): a mark for dots and glyphs,
/// a text shade for words, and the washes behind tiles, badges and links.
enum CardHue {
    case green, amber, red, blue, purple

    private var hexes: (mark: UInt32, text: UInt32, darkMark: UInt32, darkText: UInt32) {
        switch self {
        case .green: (0x2FC261, 0x1E8A4C, 0x34C77B, 0x63D9A0)
        case .amber: (0xE9A80E, 0xA5650F, 0xF2A62A, 0xFFC560)
        case .red: (0xE5484D, 0xC6283D, 0xFF5A5F, 0xFF8A8D)
        case .blue: (0x4A8CF0, 0x3B5BAB, 0x5AA9FF, 0x8CC2FF)
        case .purple: (0x8B5CF6, 0x6227AD, 0xA27BF7, 0xC4B0FF)
        }
    }

    var mark: Color { Color(light: hexes.mark, dark: hexes.darkMark) }
    var text: Color { Color(light: hexes.text, dark: hexes.darkText) }
    /// Behind a tile or a badge: pale on Paper, stronger on Night, where a pale wash disappears.
    var wash: Color { .card(hexes.mark, 0.075, hexes.darkMark, 0.13) }
    /// Behind a finished card's link.
    var tint: Color { .card(hexes.mark, 0.09, hexes.darkMark, 0.14) }
}

/// The cards' neutrals: the page's foreground at low strength, so they sit right on Paper and Night.
enum CardInk {
    private static let fg: (UInt32, UInt32) = (0x1A1A1C, 0xE4E4E7)
    static let fill = Color.card(fg.0, 0.025, fg.1, 0.025)
    static let hairline = Color.card(fg.0, 0.1, fg.1, 0.1)
    static let divider = Color.card(fg.0, 0.07, fg.1, 0.07)
    static let pill = Color.card(fg.0, 0.045, fg.1, 0.075)
    static let mono = Color.card(fg.0, 0.52, fg.1, 0.58)
    static let dot = Color.card(fg.0, 0.32, fg.1, 0.36)
    static let button = Color.card(fg.0, 0.92, fg.1, 0.92)
    static let onButton = Trek.background
    /// The chip: the page's own colour, a touch lifted on Night so the pill reads.
    static let chipFill = Color(light: 0xFFFFFF, dark: 0x131314)
    static let chipBorder = Color.card(fg.0, 0.085, fg.1, 0.12)
}

extension Color {
    /// `light` at `la` on Paper, `dark` at `da` on Night.
    static func card(_ light: UInt32, _ la: Double, _ dark: UInt32, _ da: Double) -> Color {
        Color(uiColor: UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: dark).withAlphaComponent(da) : UIColor(hex: light).withAlphaComponent(la) })
    }
}

/// One of the cards' lucide glyphs (`Assets/Cards`), as the Mac draws them.
struct CardIcon: View {
    var name: String
    var size: CGFloat

    var body: some View {
        Image("card-\(name)")
            .resizable()
            .renderingMode(.template)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

// MARK: Pieces

/// A 28 pt round tile with a 14 pt glyph: the header's and the outcome's lead.
struct CardTile: View {
    var icon: String
    var hue: CardHue?

    var body: some View {
        CardIcon(name: icon, size: 14)
            .foregroundStyle(hue?.text ?? Trek.muted)
            .frame(width: 28, height: 28)
            .background(hue?.wash ?? CardInk.pill, in: Circle())
    }
}

/// The header's badge: a pull request's state, a release's channel, a run's trigger.
struct CardBadge: View {
    var label: String
    var hue: CardHue?
    var icon: String?

    var body: some View {
        HStack(spacing: 4) {
            if let icon { CardIcon(name: icon, size: 11) }
            Text(label).font(.ui(size: 12, weight: .medium, relativeTo: .caption))
        }
        .foregroundStyle(hue?.text ?? Trek.muted)
        .padding(.horizontal, 9)
        .padding(.vertical, 3)
        .background(hue?.wash ?? CardInk.pill, in: Capsule())
        .fixedSize()
    }
}

/// A branch or commit: mono on a soft square pill.
struct MonoPill: View {
    var text: String

    var body: some View {
        Text(text)
            .font(.code(size: 11, relativeTo: .caption2))
            .foregroundStyle(CardInk.mono)
            .lineLimit(1)
            .truncationMode(.middle)
            .padding(.horizontal, 5)
            .padding(.vertical, 1)
            .background(CardInk.pill, in: RoundedRectangle(cornerRadius: 4, style: .continuous))
    }
}

/// The reference's busy mark: three quarters of a ring running round a faint track; still when
/// motion is reduced.
struct RingSpinner: View {
    var size: CGFloat
    var color: Color = Trek.muted
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var turning = false

    var body: some View {
        ZStack {
            Circle().stroke(color.opacity(0.15), lineWidth: 1.4)
            Circle()
                .trim(from: 0, to: 0.75)
                .stroke(color, style: StrokeStyle(lineWidth: 1.4, lineCap: .round))
                .rotationEffect(.degrees(turning ? 270 : -90))
        }
        .padding(0.7)
        .frame(width: size, height: size)
        .onAppear {
            guard !reduceMotion else { return }
            withAnimation(.linear(duration: 0.9).repeatForever(autoreverses: false)) { turning = true }
        }
        .accessibilityHidden(true)
    }
}

/// A check's or job's state: a green tick, a red cross, a spinner, a dotted ring (queued) or a
/// ring with a dash (skipped).
struct StateMark: View {
    var state: CheckState

    var body: some View {
        Group {
            switch state {
            case .pass: glyph("check", CardHue.green.mark)
            case .fail: glyph("x", CardHue.red.mark)
            case .running: RingSpinner(size: 15, color: CardHue.blue.mark)
            case .queued:
                Circle().strokeBorder(CardInk.dot, style: StrokeStyle(lineWidth: 1.4, dash: [2, 2])).padding(1.5)
            case .skipped:
                Circle().strokeBorder(CardInk.dot, lineWidth: 1.4).padding(1.5)
                    .overlay(Capsule().fill(Trek.muted).frame(width: 6, height: 1.4))
            }
        }
        .frame(width: 16, height: 16)
        .transition(.scale(scale: 0.6).combined(with: .opacity))
        .id(state)
    }

    private func glyph(_ name: String, _ fill: Color) -> some View {
        Circle().fill(fill)
            .overlay(CardIcon(name: name, size: 10).foregroundStyle(Trek.background).fontWeight(.bold))
    }
}

/// The section's label: "JOBS (3) · 2 of 3 passed".
struct CardSectionLabel: View {
    var head: String
    var state: String?

    var body: some View {
        HStack(spacing: 5) {
            Text(head).tracking(0.3)
            if let state, !state.isEmpty {
                Text("·")
                Text(state).contentTransition(.numericText())
            }
        }
        .font(.ui(size: 12, weight: .medium, relativeTo: .caption))
        .foregroundStyle(Trek.muted)
        .lineLimit(1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 14)
        .padding(.top, 11)
        .padding(.bottom, 3)
    }
}

/// A check or job: its mark, its name, and (jobs) how long it took.
struct CardJobRow: View {
    var state: CheckState
    var label: String
    var time: String = ""

    var body: some View {
        HStack(spacing: 10) {
            StateMark(state: state)
            Text(label)
                .font(.ui(size: 14, relativeTo: .subheadline))
                .foregroundStyle(tone)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            if !time.isEmpty {
                Text(time)
                    .font(.code(size: 12, relativeTo: .caption))
                    .foregroundStyle(state == .fail ? CardHue.red.text : Trek.muted)
                    .monospacedDigit()
            }
        }
        .padding(.horizontal, 14)
        .frame(minHeight: 34)
        .accessibilityElement(children: .combine)
        .accessibilityValue(Self.spoken(state))
    }

    private var tone: Color {
        switch state {
        case .running: CardHue.blue.text
        case .fail: CardHue.red.text
        case .queued, .skipped: Trek.muted
        case .pass: Trek.foreground
        }
    }

    static func spoken(_ s: CheckState) -> String {
        switch s {
        case .pass: "passed"
        case .fail: "failed"
        case .running: "running"
        case .queued: "queued"
        case .skipped: "skipped"
        }
    }
}

/// The card's hairline between its parts.
struct CardDivider: View {
    var body: some View { Rectangle().fill(CardInk.divider).frame(height: 0.5) }
}

// MARK: Buttons

/// A card's button: dark while it asks for a tap, light beside it, a busy pill once pressed, a
/// tinted link once there's nothing left but to look.
struct CardButton: View {
    enum Style { case dark, light, busy, link(CardHue) }
    var label: String
    var style: Style
    var icon: String? = nil
    var action: () -> Void = {}

    var body: some View {
        Button(action: action) {
            HStack(spacing: isLink ? 3 : 6) {
                switch style {
                case .busy: RingSpinner(size: 13)
                case .dark: if let icon { CardIcon(name: icon, size: 13) }
                default: EmptyView()
                }
                Text(label).font(.ui(size: 14, weight: .medium, relativeTo: .subheadline))
                if isLink { CardIcon(name: "arrow-up-right", size: 12) }
            }
            .foregroundStyle(foreground)
            .padding(.horizontal, isLink ? 12 : 16)
            .frame(minHeight: 34)
            .background(background, in: Capsule())
            .contentShape(Capsule())
            .fixedSize()
        }
        .buttonStyle(CardPress())
        .disabled(isBusy)
    }

    private var isLink: Bool { if case .link = style { true } else { false } }
    private var isBusy: Bool { if case .busy = style { true } else { false } }

    private var foreground: Color {
        switch style {
        case .dark: CardInk.onButton
        case .light: Trek.foreground.opacity(0.85)
        case .busy: Trek.muted
        case .link(let h): h.text
        }
    }

    private var background: Color {
        switch style {
        case .dark: CardInk.button
        case .light, .busy: CardInk.pill
        case .link(let h): h.tint
        }
    }
}

/// A press that dims a little and sinks a touch.
struct CardPress: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .opacity(configuration.isPressed ? 0.75 : 1)
            .scaleEffect(configuration.isPressed ? 0.97 : 1)
            .animation(.snappy(duration: 0.15), value: configuration.isPressed)
    }
}

// MARK: Layout

/// Lays its children out in rows, wrapping onto the next when a row is full (the meta line, the
/// conflicting files).
struct CardFlow: Layout {
    var spacing: CGFloat = 6
    var lineSpacing: CGFloat = 4

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let rows = rows(for: proposal.width ?? .infinity, subviews)
        let width = rows.map { $0.width }.max() ?? 0
        let height = rows.map(\.height).reduce(0, +) + lineSpacing * CGFloat(max(0, rows.count - 1))
        return CGSize(width: proposal.width ?? width, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in rows(for: bounds.width, subviews) {
            var x = bounds.minX
            for i in row.items {
                let size = subviews[i].sizeThatFits(.init(width: bounds.width, height: nil))
                subviews[i].place(at: CGPoint(x: x, y: y + (row.height - size.height) / 2), proposal: .init(width: min(size.width, bounds.width), height: size.height))
                x += size.width + spacing
            }
            y += row.height + lineSpacing
        }
    }

    private struct Row { var items: [Int] = []; var width: CGFloat = 0; var height: CGFloat = 0 }

    private func rows(for width: CGFloat, _ subviews: Subviews) -> [Row] {
        var rows: [Row] = [Row()]
        for i in subviews.indices {
            let size = subviews[i].sizeThatFits(.init(width: width, height: nil))
            let w = min(size.width, width)
            if !rows[rows.count - 1].items.isEmpty, rows[rows.count - 1].width + spacing + w > width { rows.append(Row()) }
            var row = rows[rows.count - 1]
            row.width += (row.items.isEmpty ? 0 : spacing) + w
            row.height = max(row.height, size.height)
            row.items.append(i)
            rows[rows.count - 1] = row
        }
        return rows.filter { !$0.items.isEmpty }
    }
}

/// The card's frame: the header, what's under it, the outcome, on the soft fill with a hairline,
/// as the changes card has it. A tap on the header opens it on GitHub; a long press offers the
/// link.
struct CardFrame<Body: View, Footer: View>: View {
    var card: RemoteCard
    var header: CardHeader
    @ViewBuilder var content: () -> Body
    @ViewBuilder var footer: () -> Footer
    @Environment(\.openURL) private var openURL

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                if let url = card.url.flatMap(URL.init(string:)) { openURL(url) }
            } label: {
                header.contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(card.url == nil)
            .accessibilityHint(card.url == nil ? "" : "Opens it on GitHub")
            content()
            footer()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(CardInk.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(CardInk.hairline, lineWidth: 0.75))
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .contextMenu { CardMenu(card: card) }
        .animation(.snappy(duration: 0.3), value: card)
    }
}

/// Open and copy, for the card's long press.
struct CardMenu: View {
    var card: RemoteCard
    @Environment(\.openURL) private var openURL

    var body: some View {
        if let url = card.url.flatMap(URL.init(string:)) {
            Button { openURL(url) } label: { Label("Open on GitHub", systemImage: "safari") }
            Button { UIPasteboard.general.url = url } label: { Label("Copy link", systemImage: "link") }
        }
        if let notes = card.notesUrl.flatMap(URL.init(string:)) {
            Button { openURL(notes) } label: { Label("Release notes", systemImage: "doc.text") }
        }
    }
}

/// Title, meta line and badge: what every card starts with.
struct CardHeader: View {
    var tile: (icon: String, hue: CardHue?)
    /// The strong name, then what it's about (muted for a pull request's number).
    var name: String
    var rest: String?
    var restMuted = false
    var badge: CardBadge?
    var meta: [Meta]

    enum Meta: Hashable {
        case pill(String)
        case arrow
        case text(String)
        case lines(added: Int, removed: Int)
    }

    var body: some View {
        HStack(alignment: .top, spacing: 11) {
            CardTile(icon: tile.icon, hue: tile.hue)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .top, spacing: 8) {
                    title
                        .lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    badge
                }
                CardFlow(spacing: 6, lineSpacing: 4) {
                    ForEach(meta, id: \.self) { m in
                        switch m {
                        case .pill(let s): MonoPill(text: s)
                        case .arrow: Text("→").font(.ui(size: 11, relativeTo: .caption2)).foregroundStyle(Trek.muted.opacity(0.85))
                        case .text(let s): Text(s).font(.ui(size: 13, relativeTo: .footnote)).foregroundStyle(Trek.muted).monospacedDigit()
                        case .lines(let a, let r):
                            HStack(spacing: 5) {
                                Text("+\(a)").foregroundStyle(CardHue.green.text)
                                Text("−\(r)").foregroundStyle(CardHue.red.text)
                            }
                            .font(.ui(size: 13, relativeTo: .footnote))
                        }
                    }
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 14)
        .padding(.bottom, 12)
    }

    private var title: Text {
        let strong = Text(name).font(.ui(size: 15, weight: .semibold, relativeTo: .subheadline)).foregroundStyle(Trek.foreground)
        guard let rest, !rest.isEmpty else { return strong }
        let tail = Text(" " + rest).font(.ui(size: 15, relativeTo: .subheadline))
            .foregroundStyle(restMuted ? Trek.muted : Trek.foreground.opacity(0.82))
        return Text("\(strong)\(tail)")
    }
}

/// The outcome under a card: a tile, what happened and a line about it, an action beside it when
/// there's room (or under it), and any files it names.
struct CardOutcome<Files: View, Actions: View>: View {
    var tile: (icon: String, hue: CardHue?)
    var title: String
    var detail: String
    /// One short action beside the words; else `actions` go on their own line.
    var inline: CardButton?
    @ViewBuilder var files: () -> Files
    @ViewBuilder var actions: () -> Actions

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 11) {
                CardTile(icon: tile.icon, hue: tile.hue)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .font(.ui(size: 15, weight: .medium, relativeTo: .subheadline))
                        .foregroundStyle(Trek.foreground)
                    if !detail.isEmpty {
                        Text(detail)
                            .font(.ui(size: 13, relativeTo: .footnote))
                            .foregroundStyle(Trek.muted)
                            .lineLimit(3)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentTransition(.opacity)
                inline
            }
            files()
            actions()
        }
        .padding(.horizontal, 14)
        .padding(.top, 13)
        .padding(.bottom, 14)
    }
}

extension CardOutcome where Files == EmptyView, Actions == EmptyView {
    init(tile: (icon: String, hue: CardHue?), title: String, detail: String, inline: CardButton? = nil) {
        self.init(tile: tile, title: title, detail: detail, inline: inline, files: { EmptyView() }, actions: { EmptyView() })
    }
}

/// The buttons row under an outcome, to the right.
struct CardActions<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        HStack(spacing: 8) {
            Spacer(minLength: 0)
            content()
        }
    }
}

// MARK: The card for an item

/// The card a `card` item stands for: a pull request's, a run's or a release's. `act` presses
/// one of its buttons on the Mac (nil: this Mac doesn't take them from the phone).
struct LiveCard: View {
    var card: RemoteCard
    var act: ((CardAction) -> Void)?

    var body: some View {
        switch card.kind {
        case .pr: PrCard(card: card, act: act)
        case .run: RunCard(card: card, act: act)
        case .release: ReleaseCard(card: card, act: act)
        case .other:
            Label(card.summary, systemImage: "info.circle").font(.ui(.subheadline)).foregroundStyle(Trek.muted)
        }
    }
}

/// A card in the transcript, with its buttons when this Mac takes them from the phone.
struct TranscriptCard: View {
    var card: RemoteCard
    var threadID: String
    @Environment(AppModel.self) private var model

    var body: some View {
        LiveCard(card: card, act: model.serves(.cardActions, on: threadID) ? { model.cardAction(threadID, key: card.key, $0) } : nil)
            .padding(.vertical, 2)
            .transition(.opacity.combined(with: .offset(y: 8)))
    }
}

/// The clock a card's times are told by: ticking while something's under way, still once it's done.
struct CardClock<Content: View>: View {
    var live: Bool
    @ViewBuilder var content: (Int64) -> Content

    var body: some View {
        if live {
            TimelineView(.periodic(from: .now, by: 1)) { ctx in content(RemoteCard.nowMs(ctx.date)) }
        } else {
            content(RemoteCard.nowMs())
        }
    }
}
