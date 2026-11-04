import Darwin
import Foundation

/// A lazy, per-user store. Only this versioned namespace is ever inspected;
/// legacy VoicePort-UUID folders and other cache/temp data are not candidates.
final class RecordingStorage {
    static let retention: TimeInterval = 24 * 60 * 60
    static let rootMarkerName = "owner.json"
    static let sessionMarkerName = "session.json"
    static let leaseName = "lease"
    static let audioName = "dictation.m4a"
    private static let application = "io.voiceport.recording-storage"
    let rootURL: URL
    private let clock: () -> Date

    init(rootURL: URL = URL(fileURLWithPath: NSHomeDirectory(), isDirectory: true)
        .appendingPathComponent("Library/Caches/io.voiceport.recordings-v1", isDirectory: true),
         clock: @escaping () -> Date = Date.init) {
        self.rootURL = rootURL
        self.clock = clock
    }

    struct CleanupReport: Codable {
        var removed = 0
        var active = 0
        var retained = 0
        var failed = 0
        var rootUnavailable = false
    }

    static func cleanupAtLaunch(arguments: [String],
                                makeStorage: () -> RecordingStorage = { RecordingStorage() }) -> CleanupReport? {
        guard !arguments.contains("--smoke-test") else { return nil }
        return makeStorage().cleanupExpired()
    }

    func createSession() throws -> Session {
        let createdAt = clock().timeIntervalSince1970
        guard createdAt.isFinite, createdAt >= 0 else { throw StorageError.unavailable }
        let root = try openRoot(create: true)
        let name = UUID().uuidString
        guard root.stillLinked() else { throw StorageError.unavailable }
        guard mkdirat(root.fd, name, 0o700) == 0 else { throw StorageError.unavailable }
        let directory = openat(root.fd, name, RecordingIO.directoryFlags)
        guard directory >= 0 else {
            _ = unlinkat(root.fd, name, AT_REMOVEDIR)
            throw StorageError.unavailable
        }
        var lease: Int32 = -1
        var created: [String: stat] = [:]
        do {
            guard RecordingIO.privateDirectory(try RecordingIO.info(directory)) else { throw StorageError.unavailable }
            lease = try RecordingIO.createFile(directory, Self.leaseName, data: Data())
            created[Self.leaseName] = try RecordingIO.info(lease)
            guard flock(lease, LOCK_EX | LOCK_NB) == 0 else { throw StorageError.unavailable }
            let marker = SessionMarker(application: Self.application, version: 1, ownerUID: geteuid(),
                                       kind: "session", rootID: root.rootID, sessionID: name, createdAt: createdAt)
            for (file, data) in [(Self.sessionMarkerName, try JSONEncoder().encode(marker)), (Self.audioName, Data())] {
                let fd = try RecordingIO.createFile(directory, file, data: data)
                defer { close(fd) }
                created[file] = try RecordingIO.info(fd)
            }
            guard root.stillLinked(), RecordingIO.matchesDirectory(root.fd, name, directory) else { throw StorageError.unavailable }
            return Session(root: root, directory: directory, lease: lease, name: name,
                           audioURL: rootURL.appendingPathComponent(name).appendingPathComponent(Self.audioName))
        } catch {
            // Roll back only the exact files this transaction created, never a
            // recursive removal of a path that may now contain unknown data.
            if let names = try? RecordingIO.names(directory), Set(names) == Set(created.keys),
               created.allSatisfy({ RecordingIO.matchesAt(directory, $0.key, $0.value) }) {
                for (file, info) in created where RecordingIO.matchesAt(directory, file, info) {
                    _ = unlinkat(directory, file, 0)
                }
                if RecordingIO.matchesDirectory(root.fd, name, directory) { _ = unlinkat(root.fd, name, AT_REMOVEDIR) }
            }
            if lease >= 0 { close(lease) }
            close(directory)
            throw error
        }
    }

    func cleanupExpired() -> CleanupReport {
        var report = CleanupReport()
        let now = clock().timeIntervalSince1970
        guard now.isFinite, now >= 0, let root = try? openRoot(create: false),
              let rootInfo = try? RecordingIO.info(root.fd), RecordingIO.notFuture(rootInfo, now),
              RecordingIO.notFuture(root.markerInfo, now), let names = try? RecordingIO.names(root.fd) else {
            report.rootUnavailable = true
            return report
        }
        for name in names where name != Self.rootMarkerName {
            guard Self.isIdentifier(name), root.stillLinked() else { report.retained += 1; continue }
            let directory = openat(root.fd, name, RecordingIO.directoryFlags)
            guard directory >= 0 else { report.retained += 1; continue }
            defer { close(directory) }
            guard let directoryInfo = try? RecordingIO.info(directory), RecordingIO.privateDirectory(directoryInfo),
                  let leaseInfo = try? RecordingIO.infoAt(directory, Self.leaseName),
                  RecordingIO.privateFile(leaseInfo), leaseInfo.st_size == 0,
                  RecordingIO.notFuture(leaseInfo, now) else { report.retained += 1; continue }
            let lease = openat(directory, Self.leaseName, O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
            guard lease >= 0 else { report.retained += 1; continue }
            defer { close(lease) }
            guard let openedLease = try? RecordingIO.info(lease), RecordingIO.sameFile(leaseInfo, openedLease) else {
                report.retained += 1
                continue
            }
            guard flock(lease, LOCK_EX | LOCK_NB) == 0 else {
                if errno == EWOULDBLOCK || errno == EAGAIN { report.active += 1 }
                else { report.failed += 1 }
                continue
            }
            guard let files = try? Self.inspectSession(root: root, directory: directory, lease: lease,
                                                     name: name, now: now, expired: true) else {
                report.retained += 1
                continue
            }
            if Self.removeSession(root: root, directory: directory, name: name, files: files) { report.removed += 1 }
            else { report.failed += 1 }
        }
        return report
    }

    final class Session {
        let audioURL: URL
        private let root: RecordingRoot
        private var directory: Int32
        private var lease: Int32
        private let name: String

        fileprivate init(root: RecordingRoot, directory: Int32, lease: Int32, name: String, audioURL: URL) {
            self.root = root; self.directory = directory; self.lease = lease; self.name = name; self.audioURL = audioURL
        }

        // Closing the handle releases flock even if deletion was impossible or
        // the owner disappears. A process crash releases the OS lock as well.
        deinit {
            if lease >= 0 { close(lease) }
            if directory >= 0 { close(directory) }
        }

        func secureAudioFile() throws {
            guard directory >= 0, root.stillLinked(), RecordingIO.matchesDirectory(root.fd, name, directory) else {
                throw StorageError.unavailable
            }
            let fd = openat(directory, RecordingStorage.audioName, O_RDWR | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
            guard fd >= 0 else { throw StorageError.unavailable }
            defer { close(fd) }
            let info = try RecordingIO.info(fd)
            guard RecordingIO.regularOwned(info), fchmod(fd, 0o600) == 0,
                  RecordingIO.privateFile(try RecordingIO.info(fd)),
                  RecordingIO.matchesAt(directory, RecordingStorage.audioName, try RecordingIO.info(fd)) else {
                throw StorageError.unavailable
            }
        }

        @discardableResult
        func discard() -> Bool {
            guard directory >= 0, lease >= 0 else { return true }
            defer { close(lease); close(directory); lease = -1; directory = -1 }
            guard let files = try? RecordingStorage.inspectSession(root: root, directory: directory, lease: lease,
                    name: name, now: Date().timeIntervalSince1970, expired: false) else { return false }
            return RecordingStorage.removeSession(root: root, directory: directory, name: name, files: files)
        }
    }

    private struct RootMarker: Codable {
        let application: String
        let version: Int
        let ownerUID: uid_t
        let kind: String
        let rootID: String
        static let keys: Set<String> = ["application", "version", "ownerUID", "kind", "rootID"]
    }

    private struct SessionMarker: Codable {
        let application: String
        let version: Int
        let ownerUID: uid_t
        let kind: String
        let rootID: String
        let sessionID: String
        let createdAt: TimeInterval
        static let keys: Set<String> = ["application", "version", "ownerUID", "kind", "rootID", "sessionID", "createdAt"]
    }

    private func openRoot(create: Bool) throws -> RecordingRoot {
        guard rootURL.isFileURL, rootURL.path.hasPrefix("/"),
              !rootURL.pathComponents.contains(".."), !rootURL.pathComponents.contains("."),
              rootURL.lastPathComponent != "/" else { throw StorageError.unavailable }
        // O_NOFOLLOW_ANY rejects symlinks in the entire parent path too. A
        // symlinked cache/home is conservatively unsupported, not traversed.
        let parent = open(rootURL.deletingLastPathComponent().path,
                          (RecordingIO.directoryFlags & ~O_NOFOLLOW) | O_NOFOLLOW_ANY)
        guard parent >= 0 else { throw StorageError.unavailable }
        var transferred = false
        defer { if !transferred { close(parent) } }
        var madeRoot = false
        if create {
            if mkdirat(parent, rootURL.lastPathComponent, 0o700) == 0 { madeRoot = true }
            else if errno != EEXIST { throw StorageError.unavailable }
        }
        let directory = openat(parent, rootURL.lastPathComponent, RecordingIO.directoryFlags)
        guard directory >= 0 else { throw StorageError.unavailable }
        defer { if !transferred { close(directory) } }
        guard RecordingIO.privateDirectory(try RecordingIO.info(directory)) else { throw StorageError.unavailable }
        if madeRoot {
            do {
                let marker = RootMarker(application: Self.application, version: 1, ownerUID: geteuid(), kind: "root", rootID: UUID().uuidString)
                let fd = try RecordingIO.createFile(directory, Self.rootMarkerName, data: JSONEncoder().encode(marker))
                close(fd)
            } catch {
                if RecordingIO.matchesDirectory(parent, rootURL.lastPathComponent, directory) {
                    _ = unlinkat(parent, rootURL.lastPathComponent, AT_REMOVEDIR)
                }
                throw error
            }
        }
        let (data, info) = try RecordingIO.readMarker(directory, Self.rootMarkerName, keys: RootMarker.keys)
        let marker = try JSONDecoder().decode(RootMarker.self, from: data)
        guard marker.application == Self.application, marker.version == 1, marker.kind == "root",
              marker.ownerUID == geteuid(), Self.isIdentifier(marker.rootID) else { throw StorageError.unavailable }
        let root = RecordingRoot(fd: directory, parent: parent, name: rootURL.lastPathComponent,
                                 rootID: marker.rootID, markerInfo: info)
        transferred = true
        guard root.stillLinked() else { throw StorageError.unavailable }
        return root
    }

    private static func isIdentifier(_ name: String) -> Bool { UUID(uuidString: name)?.uuidString == name }

    private static func inspectSession(root: RecordingRoot, directory: Int32, lease: Int32,
                                       name: String, now: TimeInterval, expired: Bool) throws -> [String: stat] {
        let directoryInfo = try RecordingIO.info(directory)
        guard root.stillLinked(), RecordingIO.privateDirectory(directoryInfo), RecordingIO.notFuture(directoryInfo, now),
              RecordingIO.matchesDirectory(root.fd, name, directory) else { throw StorageError.unavailable }
        let names = Set(try RecordingIO.names(directory))
        guard names.isSuperset(of: [sessionMarkerName, leaseName]),
              names.isSubset(of: [sessionMarkerName, leaseName, audioName]) else { throw StorageError.unavailable }
        var files: [String: stat] = [:]
        for name in names {
            let info = try RecordingIO.infoAt(directory, name)
            guard RecordingIO.privateFile(info), RecordingIO.notFuture(info, now) else { throw StorageError.unavailable }
            files[name] = info
        }
        guard let leaseInfo = files[leaseName], leaseInfo.st_size == 0,
              RecordingIO.sameFile(leaseInfo, try RecordingIO.info(lease)) else { throw StorageError.unavailable }
        let (data, info) = try RecordingIO.readMarker(directory, sessionMarkerName, keys: SessionMarker.keys)
        guard let expected = files[sessionMarkerName], RecordingIO.sameFile(expected, info) else { throw StorageError.unavailable }
        let marker = try JSONDecoder().decode(SessionMarker.self, from: data)
        guard marker.application == application, marker.version == 1, marker.kind == "session",
              marker.ownerUID == geteuid(), marker.rootID == root.rootID, marker.sessionID == name,
              isIdentifier(name), marker.createdAt.isFinite, marker.createdAt >= 0, marker.createdAt <= now,
              !expired || now - marker.createdAt > retention else { throw StorageError.unavailable }
        return files
    }

    private static func removeSession(root: RecordingRoot, directory: Int32, name: String, files: [String: stat]) -> Bool {
        guard root.stillLinked(), RecordingIO.matchesDirectory(root.fd, name, directory),
              let names = try? RecordingIO.names(directory), Set(names) == Set(files.keys),
              files.allSatisfy({ RecordingIO.matchesAt(directory, $0.key, $0.value) }) else { return false }
        // Descriptor-relative unlink never follows a file/dir symlink and never
        // recursively deletes mixed content. Revalidate each inode before unlink.
        // These checks are not atomic against hostile mutation by the same UID.
        for file in [audioName, sessionMarkerName, leaseName] {
            guard let info = files[file] else { continue }
            guard root.stillLinked(), RecordingIO.matchesDirectory(root.fd, name, directory),
                  RecordingIO.matchesAt(directory, file, info), unlinkat(directory, file, 0) == 0 else { return false }
        }
        guard RecordingIO.matchesDirectory(root.fd, name, directory) else { return false }
        return unlinkat(root.fd, name, AT_REMOVEDIR) == 0
    }

    enum StorageError: LocalizedError {
        case unavailable
        var errorDescription: String? { "无法安全创建录音临时文件，请检查本用户缓存目录是否可用。" }
    }
}

fileprivate final class RecordingRoot {
    let fd: Int32
    let parent: Int32
    let name: String
    let rootID: String
    let markerInfo: stat

    init(fd: Int32, parent: Int32, name: String, rootID: String, markerInfo: stat) {
        self.fd = fd; self.parent = parent; self.name = name; self.rootID = rootID; self.markerInfo = markerInfo
    }
    deinit { close(fd); close(parent) }
    func stillLinked() -> Bool {
        RecordingIO.matchesDirectory(parent, name, fd)
            && ((try? RecordingIO.info(fd)).map(RecordingIO.privateDirectory) ?? false)
            && RecordingIO.matchesAt(fd, RecordingStorage.rootMarkerName, markerInfo)
    }
}

private enum RecordingIO {
    static let directoryFlags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC
    static func info(_ fd: Int32) throws -> stat {
        var info = stat()
        guard fstat(fd, &info) == 0 else { throw RecordingStorage.StorageError.unavailable }
        return info
    }
    static func infoAt(_ fd: Int32, _ name: String) throws -> stat {
        var info = stat()
        guard fstatat(fd, name, &info, AT_SYMLINK_NOFOLLOW) == 0 else { throw RecordingStorage.StorageError.unavailable }
        return info
    }
    static func privateDirectory(_ info: stat) -> Bool {
        info.st_mode & S_IFMT == S_IFDIR && info.st_uid == geteuid() && info.st_mode & 0o7777 == 0o700
    }
    static func regularOwned(_ info: stat) -> Bool { info.st_mode & S_IFMT == S_IFREG && info.st_uid == geteuid() && info.st_nlink == 1 }
    static func privateFile(_ info: stat) -> Bool { regularOwned(info) && info.st_mode & 0o7777 == 0o600 }
    static func notFuture(_ info: stat, _ now: TimeInterval) -> Bool {
        Double(info.st_mtimespec.tv_sec) + Double(info.st_mtimespec.tv_nsec) / 1_000_000_000 <= now
    }
    static func sameFile(_ a: stat, _ b: stat) -> Bool {
        a.st_dev == b.st_dev && a.st_ino == b.st_ino && a.st_mode == b.st_mode && a.st_uid == b.st_uid
            && a.st_nlink == b.st_nlink && a.st_size == b.st_size
            && a.st_mtimespec.tv_sec == b.st_mtimespec.tv_sec && a.st_mtimespec.tv_nsec == b.st_mtimespec.tv_nsec
    }
    static func matchesAt(_ fd: Int32, _ name: String, _ expected: stat) -> Bool {
        (try? infoAt(fd, name)).map { sameFile($0, expected) } ?? false
    }
    static func matchesDirectory(_ parent: Int32, _ name: String, _ fd: Int32) -> Bool {
        guard let a = try? infoAt(parent, name), let b = try? info(fd) else { return false }
        return a.st_mode & S_IFMT == S_IFDIR && a.st_dev == b.st_dev && a.st_ino == b.st_ino
    }
    static func names(_ fd: Int32) throws -> [String] {
        let listingFD = openat(fd, ".", directoryFlags)
        guard listingFD >= 0 else { throw RecordingStorage.StorageError.unavailable }
        guard let listing = fdopendir(listingFD) else { close(listingFD); throw RecordingStorage.StorageError.unavailable }
        defer { closedir(listing) }
        var names: [String] = []
        errno = 0
        while let entry = readdir(listing) {
            var bytes = entry.pointee.d_name
            let capacity = MemoryLayout.size(ofValue: bytes)
            let name = withUnsafePointer(to: &bytes) { $0.withMemoryRebound(to: CChar.self, capacity: capacity) { String(cString: $0) } }
            if name != "." && name != ".." { names.append(name) }
            guard names.count <= 1024 else { throw RecordingStorage.StorageError.unavailable }
            errno = 0
        }
        guard errno == 0 else { throw RecordingStorage.StorageError.unavailable }
        return names
    }
    static func createFile(_ directory: Int32, _ name: String, data: Data) throws -> Int32 {
        let fd = openat(directory, name, O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw RecordingStorage.StorageError.unavailable }
        var complete = false
        defer {
            if !complete {
                if let info = try? info(fd), matchesAt(directory, name, info) { _ = unlinkat(directory, name, 0) }
                close(fd)
            }
        }
        guard fchmod(fd, 0o600) == 0 else { throw RecordingStorage.StorageError.unavailable }
        try data.withUnsafeBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                let count = write(fd, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                if count < 0 && errno == EINTR { continue }
                guard count > 0 else { throw RecordingStorage.StorageError.unavailable }
                offset += count
            }
        }
        guard privateFile(try info(fd)) else { throw RecordingStorage.StorageError.unavailable }
        complete = true
        return fd
    }
    static func readMarker(_ directory: Int32, _ name: String, keys: Set<String>) throws -> (Data, stat) {
        let fd = openat(directory, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        guard fd >= 0 else { throw RecordingStorage.StorageError.unavailable }
        defer { close(fd) }
        let initial = try info(fd)
        guard privateFile(initial), initial.st_size > 0, initial.st_size <= 4096 else { throw RecordingStorage.StorageError.unavailable }
        var data = Data(count: Int(initial.st_size))
        try data.withUnsafeMutableBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                let count = read(fd, buffer.baseAddress!.advanced(by: offset), buffer.count - offset)
                if count < 0 && errno == EINTR { continue }
                guard count > 0 else { throw RecordingStorage.StorageError.unavailable }
                offset += count
            }
        }
        guard sameFile(initial, try info(fd)), matchesAt(directory, name, initial),
              let object = try JSONSerialization.jsonObject(with: data) as? [String: Any], Set(object.keys) == keys else {
            throw RecordingStorage.StorageError.unavailable
        }
        return (data, initial)
    }
}
