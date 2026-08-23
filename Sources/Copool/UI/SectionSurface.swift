import SwiftUI
#if canImport(AppKit)
import AppKit
#endif

struct SectionCard<Content: View, HeaderTrailing: View>: View {
    let title: String
    @ViewBuilder let headerTrailing: HeaderTrailing
    @ViewBuilder let content: Content

    init(title: String, @ViewBuilder content: () -> Content) where HeaderTrailing == EmptyView {
        self.title = title
        self.headerTrailing = EmptyView()
        self.content = content()
    }

    init(
        title: String,
        @ViewBuilder headerTrailing: () -> HeaderTrailing,
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.headerTrailing = headerTrailing()
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 12) {
                Text(title)
                    .font(.headline)
                Spacer(minLength: 0)
                headerTrailing
            }
            content
        }
        .padding(16)
        .cardSurface(cornerRadius: LayoutRules.cardRadius)
    }
}

struct CollapseChevronButton: View {
    let isExpanded: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
        }
        .liquidGlassActionButtonStyle(density: .compact)
    }
}

struct CloseGlassButton: View {
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "xmark")
        }
        .accessibilityLabel(L10n.tr("common.close"))
        .liquidGlassActionButtonStyle(density: .compact)
    }
}

struct LanguageMenuButton<Label: View>: View {
    let currentLocale: AppLocale
    let onSelectLocale: (AppLocale) -> Void
    @ViewBuilder let label: Label

    init(
        currentLocale: AppLocale,
        onSelectLocale: @escaping (AppLocale) -> Void,
        @ViewBuilder label: () -> Label
    ) {
        self.currentLocale = currentLocale
        self.onSelectLocale = onSelectLocale
        self.label = label()
    }

    var body: some View {
        Menu {
            ForEach(AppLocale.allCases) { locale in
                Button {
                    onSelectLocale(locale)
                } label: {
                    HStack {
                        Text(L10n.tr(locale.displayNameKey))
                        if locale == currentLocale {
                            Image(systemName: "checkmark")
                        }
                    }
                }
            }
        } label: {
            label
        }
        .accessibilityLabel(Text("settings.language"))
    }
}

struct ToolbarIconLabel: View {
    let systemImage: String
    var isSpinning = false
    var opticalScale = CGFloat(1)

    var body: some View {
        baseIcon
            .modifier(ToolbarIconSpinModifier(isSpinning: isSpinning))
    }

    private var baseIcon: some View {
        Image(systemName: systemImage)
            .font(.system(size: LayoutRules.toolbarIconPointSize, weight: .semibold))
            .foregroundStyle(.primary)
            .scaleEffect(opticalScale)
    }
}

private struct ToolbarIconSpinModifier: ViewModifier {
    let isSpinning: Bool

    func body(content: Content) -> some View {
        if #available(iOS 18.0, macOS 15.0, *) {
            content
                .symbolEffect(.rotate.byLayer, options: .repeating, isActive: isSpinning)
        } else {
            content
                .rotationEffect(.degrees(isSpinning ? 360 : 0))
                .animation(
                    isSpinning
                        ? .linear(duration: 1).repeatForever(autoreverses: false)
                        : .easeOut(duration: 0.2),
                    value: isSpinning
                )
        }
    }
}

struct CardSurfaceModifier: ViewModifier {
    let cornerRadius: CGFloat
    let tint: Color?

    func body(content: Content) -> some View {
        content
            .background { backgroundSurface }
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .strokeBorder(separatorColor, lineWidth: 1)
            )
    }

    @ViewBuilder
    private var backgroundSurface: some View {
        if #available(iOS 26.0, macOS 26.0, *) {
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .fill(.clear)
                .glassEffect(
                    tint == nil ? .regular : .regular.tint(tint!.opacity(0.28)),
                    in: .rect(cornerRadius: cornerRadius)
                )
        } else {
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .fill(.regularMaterial)
        }
    }

    private var separatorColor: Color {
        #if canImport(AppKit)
        Color(nsColor: .separatorColor).opacity(0.9)
        #else
        Color.secondary.opacity(0.22)
        #endif
    }
}

enum FrostedChromeTokens {
    static var separatorColor: Color {
        #if canImport(AppKit)
        Color(nsColor: .separatorColor)
        #else
        Color.secondary.opacity(0.2)
        #endif
    }

    static func tintedGlass(prominent: Bool, tint: Color?) -> Color {
        if let tint {
            return tint.opacity(prominent ? 0.22 : 0.14)
        }
        return Color.white.opacity(prominent ? 0.06 : 0.03)
    }
}

struct FrostedCapsuleSurfaceModifier: ViewModifier {
    let prominent: Bool
    let tint: Color?

    func body(content: Content) -> some View {
        content
            .background { backgroundSurface }
            .overlay {
                Capsule()
                    .strokeBorder(FrostedChromeTokens.separatorColor.opacity(prominent ? 0.85 : 1), lineWidth: 1)
            }
    }

    @ViewBuilder
    private var backgroundSurface: some View {
        if #available(iOS 26.0, macOS 26.0, *) {
            Capsule()
                .fill(.clear)
                .glassEffect(
                    .regular.tint(FrostedChromeTokens.tintedGlass(prominent: prominent, tint: tint)),
                    in: .capsule
                )
        } else {
            Capsule()
                .fill(prominent ? AnyShapeStyle(.regularMaterial) : AnyShapeStyle(.ultraThinMaterial))
        }
    }
}

struct FrostedRoundedSurfaceModifier: ViewModifier {
    let cornerRadius: CGFloat
    let prominent: Bool
    let tint: Color?

    func body(content: Content) -> some View {
        content
            .background { backgroundSurface }
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .strokeBorder(FrostedChromeTokens.separatorColor.opacity(prominent ? 0.85 : 1), lineWidth: 1)
                    .allowsHitTesting(false)
            }
    }

    @ViewBuilder
    private var backgroundSurface: some View {
        if #available(iOS 26.0, macOS 26.0, *) {
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .fill(.clear)
                .glassEffect(
                    .regular.tint(FrostedChromeTokens.tintedGlass(prominent: prominent, tint: tint)),
                    in: .rect(cornerRadius: cornerRadius)
                )
        } else {
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .fill(prominent ? AnyShapeStyle(.regularMaterial) : AnyShapeStyle(.ultraThinMaterial))
        }
    }
}

struct FrostedRoundedInputModifier: ViewModifier {
    let cornerRadius: CGFloat

    func body(content: Content) -> some View {
        content
            .textFieldStyle(.plain)
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .frostedRoundedSurface(cornerRadius: cornerRadius)
    }
}

struct GlassSelectableCardModifier: ViewModifier {
    let selected: Bool
    let cornerRadius: CGFloat
    let tint: Color

    func body(content: Content) -> some View {
        content
            .cardSurface(
                cornerRadius: cornerRadius,
                tint: selected ? tint.opacity(0.16) : nil
            )
            .overlay {
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .strokeBorder(
                        selected
                            ? tint.opacity(0.44)
                            : FrostedChromeTokens.separatorColor.opacity(0.7),
                        lineWidth: 1
                    )
            }
    }
}
