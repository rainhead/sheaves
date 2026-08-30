import Foundation
import IOKit.ps
import SheavesCore

/// Answers `TimeTracker`'s power question from the power-sources IOKit API.
///
/// `IOPSGetProvidingPowerSourceType` names whatever is actually providing power
/// right now — a desktop, or a laptop on its charger, answers AC — so the probe
/// slows the moment the cable comes out, not when the battery runs low.
enum PowerSource {
    static func current() -> PowerState {
        if ProcessInfo.processInfo.isLowPowerModeEnabled { return .lowPower }
        guard let type = IOPSGetProvidingPowerSourceType(nil)?.takeRetainedValue() as String? else {
            return .pluggedIn
        }
        return type == kIOPMBatteryPowerKey ? .battery : .pluggedIn
    }
}
