import SwiftUI
import StreamiumKit
#if os(iOS)
import AVFoundation
#endif

@main
struct StreamiumApp: App {
    @StateObject private var library = Library()
    @StateObject private var playback = PlaybackController()
    @AppStorage("acceptedUsageNotice") private var acceptedUsageNotice = false

    init() {
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .moviePlayback)
        try? AVAudioSession.sharedInstance().setActive(true)
        #endif
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(library)
                .environmentObject(playback)
                .sheet(isPresented: Binding(
                    get: { !acceptedUsageNotice },
                    set: { showing in if !showing { acceptedUsageNotice = true } }
                )) {
                    UsageNoticeView { acceptedUsageNotice = true }
                        .interactiveDismissDisabled()
                }
        }
        #if os(macOS)
        .defaultSize(width: 1100, height: 720)
        #endif
    }
}
