import AppKit
import SheavesCore
import SwiftUI

/// Asks what to do about a timer that ran while nobody was here.
///
/// It never decides on its own. Rewriting somebody's hours without being asked
/// would be a worse fault than the one it is fixing, so the absence is offered and
/// the answer is the user's.
@MainActor
final class AbsencePromptController {
    private let tracker: TimeTracker
    private var panel: NSPanel?

    /// The absence currently on screen. Nil means the next one may be asked about.
    private(set) var pending: Absence?

    /// Called once the answer has been applied, or the question waved away — the
    /// moment it is safe to ask about the next absence against settled state.
    var onFinished: (() -> Void)?

    init(tracker: TimeTracker) {
        self.tracker = tracker
    }

    func present(_ absence: Absence, for entry: TrackedEntry) {
        let root = AbsencePromptView(
            absence: absence,
            entry: entry,
            onResolve: { [weak self] resolution in
                guard let self else { return }
                // The panel goes at once — a click should not wait on the network —
                // but the next question waits for this answer to be applied, so it
                // is asked against the entry this one leaves behind.
                self.dismiss()
                Task {
                    await self.tracker.resolve(absence, on: entry, as: resolution)
                    self.onFinished?()
                }
            },
            onDismiss: { [weak self] in
                self?.dismiss()
                self?.onFinished?()
            }
        )
        .environment(tracker)
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))

        let hosting = NSHostingController(rootView: AnyView(root))
        hosting.sizingOptions = [.preferredContentSize]

        let panel = self.panel ?? makePanel()
        panel.contentViewController = hosting
        self.panel = panel
        pending = absence

        // Size before positioning: where a centred panel goes depends on how tall
        // its content turned out to be, and SwiftUI reports nothing until laid out.
        hosting.view.layoutSubtreeIfNeeded()
        let fitted = hosting.view.fittingSize
        if fitted.width > 0, fitted.height > 0 {
            panel.setContentSize(fitted)
        }
        panel.centreOnScreenUnderPointer()
        NSApp.activate(ignoringOtherApps: true)
        panel.makeKeyAndOrderFront(nil)
    }

    func dismiss() {
        panel?.orderOut(nil)
        pending = nil
    }

    private func makePanel() -> NSPanel {
        let panel = FloatingPanel(
            contentRect: NSRect(x: 0, y: 0, width: 460, height: 240),
            styleMask: [.borderless, .nonactivatingPanel, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.isMovableByWindowBackground = true
        panel.level = .floating
        // Unlike quick entry, this one is a question. Clicking away to check a
        // calendar must not throw it away unanswered.
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        return panel
    }
}

struct AbsencePromptView: View {
    @Environment(TimeTracker.self) private var tracker

    let absence: Absence
    let entry: TrackedEntry
    var onResolve: (AbsenceResolution) -> Void
    var onDismiss: () -> Void

    @State private var isChoosingTarget = false

    private var format: HoursFormat { HoursFormat(company: tracker.company) }
    private var awayHours: Double { absence.duration / 3600 }
    private var trimmedHours: Double? { absence.trimmedHours(for: entry) }

    /// The entry belongs to a day that is over, so carrying on would bank today's
    /// work onto yesterday's date. Stopping is then the only sound default.
    private var dayIsOver: Bool { entry.spentDate != .today() }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            Divider()
            if isChoosingTarget {
                TargetPicker(title: "Log the time away to…", maxVisible: 5) { target in
                    onResolve(.trimAndLog(target))
                }
            } else {
                actions
            }
        }
        .padding(16)
        .frame(width: 460, alignment: .topLeading)
        .fixedSize(horizontal: false, vertical: true)
        .onExitCommand(perform: onDismiss)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "moon.zzz.fill")
                .font(.title2)
                .foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 3) {
                Text("This timer ran while you were away")
                    .font(.headline)
                Text("\(entry.task.name) · \(entry.target.projectLabel)")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                Text(awaySummary)
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
    }

    private var awaySummary: String {
        // A date as well as a time once the absence has crossed midnight: "since
        // 5:12 PM" is a riddle when you are reading it the following morning.
        let since = absence.began.formatted(
            date: dayIsOver ? .abbreviated : .omitted,
            time: .shortened
        )
        // "Away since 12:36 PM · 2:00" reads as two clock times. Saying how long
        // first leaves nothing to misread.
        return "Away for \(awayHours.formattedHours(format)), since \(since)"
    }

    @ViewBuilder
    private var actions: some View {
        VStack(spacing: 8) {
            if dayIsOver {
                action("Trim and stop", detail: trimDetail, prominent: true) {
                    onResolve(.trimAndStop)
                }
                action("Trim, keep timing", detail: "Carries on from when you came back") {
                    onResolve(.trimAndContinue)
                }
            } else {
                action("Trim, keep timing", detail: trimDetail, prominent: true) {
                    onResolve(.trimAndContinue)
                }
                action("Trim and stop", detail: "Ends the entry where you left") {
                    onResolve(.trimAndStop)
                }
            }
            // Say what happens to the timer it is taken from: this keeps timing,
            // except on a day that is over, where it stops for the same reason
            // "Trim and stop" leads there.
            action(
                "Log the time away separately…",
                detail: dayIsOver
                    ? "Books it to the meeting you were in, and stops this timer"
                    : "Books it to the meeting you were in, and keeps timing"
            ) {
                isChoosingTarget = true
            }
            action("Keep it", detail: "It was working time after all") {
                onResolve(.keep)
            }
        }
    }

    @ViewBuilder
    private func action(
        _ title: String,
        detail: String,
        prominent: Bool = false,
        perform: @escaping () -> Void
    ) -> some View {
        let button = Button(action: perform) {
            HStack {
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                    Text(detail)
                        .font(.caption)
                        // Not a hardcoded white for the prominent one: the style
                        // already sets the foreground, and an unfocused panel paints
                        // it grey, where white text all but disappears.
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }
            .contentShape(.rect)
            .padding(.vertical, 2)
        }
        .controlSize(.large)

        // `buttonStyle` takes a concrete type, so the choice has to be made here
        // rather than passed along as a value.
        if prominent {
            button.buttonStyle(.borderedProminent)
        } else {
            button.buttonStyle(.bordered)
        }
    }

    private var trimDetail: String {
        guard let trimmedHours else { return "Takes the time away off this entry" }
        return "Leaves \(trimmedHours.formattedHours(format)) on this entry"
    }
}
