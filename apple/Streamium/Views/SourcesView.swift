import SwiftUI
import StreamiumKit

struct SourcesView: View {
    @EnvironmentObject private var library: Library
    @State private var showAdd = false

    var body: some View {
        List {
            if library.sources.isEmpty {
                ContentUnavailableView {
                    Label("No sources yet", systemImage: "server.rack")
                } description: {
                    Text("Add an Xtream account from your provider, a playlist you are entitled to use, or browse your own files under Library.")
                } actions: {
                    Button("Add a Source") { showAdd = true }
                        .buttonStyle(.borderedProminent)
                }
            }
            if !library.sources.isEmpty {
                Section("Playback") {
                    Toggle("Prefer HLS for Xtream live channels", isOn: $library.preferHLSForXtream)
                    Text("Transport streams give the lowest latency and are the default. Turn this on if a provider only serves HLS, then refresh.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
            ForEach(library.sources) { source in
                VStack(alignment: .leading, spacing: 4) {
                    Text(source.name).font(.headline)
                    Text(source.location).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    if let error = source.lastError {
                        Text(error).font(.caption).foregroundStyle(.red)
                    } else if let status = source.status {
                        Text(status).font(.caption).foregroundStyle(.secondary)
                    }
                    if let date = source.lastRefreshed, source.lastError == nil {
                        Text("Updated \(date.formatted(.relative(presentation: .named)))")
                            .font(.caption2).foregroundStyle(.tertiary)
                    }
                }
            }
            .onDelete { offsets in
                for i in offsets { library.remove(library.sources[i]) }
            }
        }
        .navigationTitle("Sources")
        .toolbar {
            ToolbarItem {
                Button { showAdd = true } label: { Label("Add", systemImage: "plus") }
            }
            ToolbarItem {
                Button {
                    Task { await library.refreshAll() }
                } label: {
                    if library.isRefreshing { ProgressView().controlSize(.small) } else { Label("Refresh", systemImage: "arrow.clockwise") }
                }
                .disabled(library.isRefreshing)
            }
            ToolbarItem {
                if library.isLoadingGuide {
                    Label("Loading guide", systemImage: "clock.arrow.circlepath")
                        .labelStyle(.iconOnly).foregroundStyle(.secondary)
                }
            }
        }
        .sheet(isPresented: $showAdd) { AddSourceView() }
    }
}

struct AddSourceView: View {
    enum Kind: String, CaseIterable { case playlist = "Playlist (M3U)", xtream = "Xtream account" }

    @EnvironmentObject private var library: Library
    @Environment(\.dismiss) private var dismiss
    @State private var kind: Kind = .playlist
    @State private var name = ""
    @State private var url = ""
    @State private var epgURL = ""
    @State private var server = ""
    @State private var username = ""
    @State private var password = ""
    @State private var isSaving = false
    @State private var error: String?

    var body: some View {
        NavigationStack {
            Form {
                Picker("Type", selection: $kind) {
                    ForEach(Kind.allCases, id: \.self) { Text($0.rawValue) }
                }
                .pickerStyle(.segmented)

                Section("Details") {
                    TextField("Name (optional)", text: $name)
                    switch kind {
                    case .playlist:
                        TextField("Playlist URL", text: $url)
                            .autocorrectionDisabled()
                        TextField("Guide (XMLTV) URL, optional", text: $epgURL)
                            .autocorrectionDisabled()
                    case .xtream:
                        TextField("Server address", text: $server)
                            .autocorrectionDisabled()
                            .onChange(of: server) { _, value in absorbCredentials(from: value) }
                        TextField("Username", text: $username)
                            .autocorrectionDisabled()
                        SecureField("Password", text: $password)
                        Text("Paste the whole URL your provider gave you and the username and password are filled in for you.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                }

                Section {
                    Text("Only add sources you are entitled to use. Credentials are stored in the Keychain on this device.")
                        .font(.footnote).foregroundStyle(.secondary)
                }

                if let error {
                    Section { Text(error).foregroundStyle(.red) }
                }
            }
            .navigationTitle("Add Source")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") { save() }.disabled(!isValid || isSaving)
                }
            }
            .overlay { if isSaving { ProgressView("Loading…") } }
        }
        #if os(macOS)
        .frame(minWidth: 480, minHeight: 360)
        #endif
    }

    /// Providers usually hand out one link such as
    /// `http://host:8080/get.php?username=abc&password=def&type=m3u_plus`.
    /// Pasting it into the address field should not then require retyping the
    /// credentials that are sitting right there in the query string.
    private func absorbCredentials(from text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.localizedCaseInsensitiveContains("username="),
              var components = URLComponents(string: trimmed) else { return }

        let items = components.queryItems ?? []
        func value(_ name: String) -> String? {
            items.first { $0.name.caseInsensitiveCompare(name) == .orderedSame }?
                .value?
                .trimmingCharacters(in: .whitespaces)
        }
        if let found = value("username"), !found.isEmpty { username = found }
        if let found = value("password"), !found.isEmpty { password = found }

        // Reduce the address to the server root. Dropping the query also stops
        // this from running again on the next change.
        components.query = nil
        components.fragment = nil
        for script in ["/get.php", "/player_api.php", "/panel_api.php", "/xmltv.php"]
        where components.path.hasSuffix(script) {
            components.path = String(components.path.dropLast(script.count))
        }
        if let cleaned = components.string, cleaned != text { server = cleaned }
    }

    private var isValid: Bool {
        switch kind {
        case .playlist: return !url.trimmingCharacters(in: .whitespaces).isEmpty
        case .xtream: return !server.isEmpty && !username.isEmpty && !password.isEmpty
        }
    }

    private func save() {
        isSaving = true
        error = nil
        Task {
            do {
                switch kind {
                case .playlist:
                    try await library.addPlaylist(name: name, url: url, epgURL: epgURL.isEmpty ? nil : epgURL)
                case .xtream:
                    try await library.addXtream(name: name, server: server, username: username, password: password)
                }
                dismiss()
            } catch {
                self.error = error.localizedDescription
            }
            isSaving = false
        }
    }
}
