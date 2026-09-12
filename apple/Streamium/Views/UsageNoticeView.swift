import SwiftUI

/// One-time notice shown on first launch. Plain language, no dark patterns.
struct UsageNoticeView: View {
    var onAccept: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Welcome to Streamium").font(.title.bold())
            Text("Streamium is a media player. It contains no channels, playlists or servers.")
            Text("It plays media from sources you add: files you own, media servers and tuners you run, and services you are subscribed to. Only add sources you are allowed to use.")
            Text("Nothing about your sources or what you watch leaves this device.")
                .foregroundStyle(.secondary)
            Spacer()
            Button(action: onAccept) {
                Text("I understand").frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
        }
        .padding(24)
        .frame(minWidth: 360, idealWidth: 480, minHeight: 320)
    }
}
