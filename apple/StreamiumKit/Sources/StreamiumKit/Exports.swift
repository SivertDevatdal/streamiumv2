// The core types (FfiChannel, FfiProgramme, ChannelCatalog, ...) are part of
// StreamiumKit's own API surface: every view that shows a channel needs them.
// Re-exporting here means `import StreamiumKit` is enough, rather than each
// file having to import both modules.
@_exported import StreamiumCore
