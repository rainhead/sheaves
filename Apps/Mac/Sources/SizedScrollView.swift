import SwiftUI

/// A scrolling list that is as tall as its content, up to `maxHeight`.
///
/// A bare `ScrollView` disappears inside a popover that sizes itself to its content:
/// the scroll view reports no minimum height, the popover offers it none, and it
/// renders at zero. `.frame(maxHeight:)` sets a ceiling, not a floor, so it does not
/// help. Measuring the content and asking for that height — capped — gives a list
/// that grows with its rows and starts scrolling only once it has to.
struct SizedScrollView<Content: View>: View {
    var maxHeight: CGFloat
    @ViewBuilder var content: Content

    @State private var contentHeight: CGFloat = 0

    var body: some View {
        ScrollView {
            content
                .onGeometryChange(for: CGFloat.self) { proxy in
                    proxy.size.height
                } action: { height in
                    contentHeight = height
                }
        }
        .frame(height: min(contentHeight, maxHeight))
        // Bouncing a list that already fits looks broken.
        .scrollDisabled(contentHeight <= maxHeight)
    }
}
