import Testing
import SheavesCore

@Suite("Rust core")
struct RustCoreTests {
    /// Calling across the boundary at all is the test: the generated Swift checks
    /// the library's interface checksums on first use and traps if the bindings
    /// were generated from a different build of the core.
    @Test("reports its name and version through the bindings")
    func version() {
        #expect(coreVersion().hasPrefix("sheaves-core "))
    }
}
