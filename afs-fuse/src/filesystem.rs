use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    Errno, FileHandle, FileType, Filesystem, FopenFlags, Generation, INodeNo, LockOwner,
    MountOption, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory,
    ReplyEmpty, ReplyEntry, ReplyWrite, Request, SessionACL, WriteFlags,
};
use tokio::runtime::Handle;

use afs_storage::StorageBackend;

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: INodeNo = INodeNo(1);

/// Permission level for a mounted directory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MountPermission {
    ReadOnly,
    ReadWrite,
}

/// FUSE filesystem backed by a StorageBackend.
pub struct AfsFilesystem {
    backend: Arc<dyn StorageBackend>,
    dir_id: String,
    permission: MountPermission,
    handle: Handle,
    inodes: std::sync::Mutex<InodeTable>,
}

struct InodeTable {
    next_ino: u64,
    ino_to_path: HashMap<u64, PathBuf>,
    path_to_ino: HashMap<PathBuf, u64>,
}

impl InodeTable {
    fn new() -> Self {
        let mut table = Self {
            next_ino: 2,
            ino_to_path: HashMap::new(),
            path_to_ino: HashMap::new(),
        };
        table.ino_to_path.insert(1, PathBuf::from(""));
        table.path_to_ino.insert(PathBuf::from(""), 1);
        table
    }

    fn get_or_create(&mut self, path: PathBuf) -> INodeNo {
        if let Some(&ino) = self.path_to_ino.get(&path) {
            return INodeNo(ino);
        }
        let ino = self.next_ino;
        self.next_ino += 1;
        self.ino_to_path.insert(ino, path.clone());
        self.path_to_ino.insert(path, ino);
        INodeNo(ino)
    }

    fn get_path(&self, ino: INodeNo) -> Option<&PathBuf> {
        self.ino_to_path.get(&ino.0)
    }
}

impl AfsFilesystem {
    pub fn new(
        backend: Arc<dyn StorageBackend>,
        dir_id: String,
        permission: MountPermission,
        handle: Handle,
    ) -> Self {
        Self {
            backend,
            dir_id,
            permission,
            handle,
            inodes: std::sync::Mutex::new(InodeTable::new()),
        }
    }

    fn is_readonly(&self) -> bool {
        self.permission == MountPermission::ReadOnly
    }

    fn storage_attr_to_fuse(&self, attr: &afs_storage::FileAttr, ino: INodeNo) -> fuser::FileAttr {
        let kind = if attr.is_dir {
            FileType::Directory
        } else {
            FileType::RegularFile
        };
        let perm = if attr.is_dir { 0o755 } else { 0o644 };
        fuser::FileAttr {
            ino,
            size: attr.size,
            blocks: (attr.size + 511) / 512,
            atime: attr.accessed,
            mtime: attr.modified,
            ctime: attr.modified,
            crtime: attr.created,
            kind,
            perm,
            nlink: if attr.is_dir { 2 } else { 1 },
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }

    fn root_attr(&self) -> fuser::FileAttr {
        fuser::FileAttr {
            ino: ROOT_INO,
            size: 0,
            blocks: 0,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }

    pub fn mount_config() -> fuser::Config {
        let mut config = fuser::Config::default();
        config.mount_options = vec![
            MountOption::FSName("afs".to_string()),
            MountOption::AutoUnmount,
        ];
        config.acl = SessionACL::All;
        config
    }
}

impl Filesystem for AfsFilesystem {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let inodes = self.inodes.lock().unwrap();
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        let child_path = parent_path.join(name);
        match self
            .handle
            .block_on(self.backend.stat(&self.dir_id, &child_path))
        {
            Ok(attr) => {
                let mut inodes = self.inodes.lock().unwrap();
                let ino = inodes.get_or_create(child_path);
                let fuse_attr = self.storage_attr_to_fuse(&attr, ino);
                reply.entry(&TTL, &fuse_attr, Generation(0));
            }
            Err(_) => {
                reply.error(Errno::ENOENT);
            }
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        if ino == ROOT_INO {
            reply.attr(&TTL, &self.root_attr());
            return;
        }

        let inodes = self.inodes.lock().unwrap();
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        match self
            .handle
            .block_on(self.backend.stat(&self.dir_id, &path))
        {
            Ok(attr) => {
                let fuse_attr = self.storage_attr_to_fuse(&attr, ino);
                reply.attr(&TTL, &fuse_attr);
            }
            Err(_) => {
                reply.error(Errno::ENOENT);
            }
        }
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let inodes = self.inodes.lock().unwrap();
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        match self
            .handle
            .block_on(self.backend.read_file(&self.dir_id, &path))
        {
            Ok(data) => {
                let offset = offset as usize;
                let end = std::cmp::min(offset + size as usize, data.len());
                if offset < data.len() {
                    reply.data(&data[offset..end]);
                } else {
                    reply.data(&[]);
                }
            }
            Err(_) => {
                reply.error(Errno::EIO);
            }
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        if self.is_readonly() {
            reply.error(Errno::EACCES);
            return;
        }

        let inodes = self.inodes.lock().unwrap();
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        let existing = self
            .handle
            .block_on(self.backend.read_file(&self.dir_id, &path))
            .unwrap_or_default();

        let offset = offset as usize;
        let mut new_data = existing;
        if offset + data.len() > new_data.len() {
            new_data.resize(offset + data.len(), 0);
        }
        new_data[offset..offset + data.len()].copy_from_slice(data);

        match self
            .handle
            .block_on(self.backend.write_file(&self.dir_id, &path, &new_data))
        {
            Ok(()) => reply.written(data.len() as u32),
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let inodes = self.inodes.lock().unwrap();
        let path = match inodes.get_path(ino) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        let entries = match self
            .handle
            .block_on(self.backend.list_dir(&self.dir_id, &path))
        {
            Ok(e) => e,
            Err(_) => {
                reply.error(Errno::EIO);
                return;
            }
        };

        let mut full_entries: Vec<(INodeNo, FileType, String)> = vec![
            (ROOT_INO, FileType::Directory, ".".to_string()),
            (ROOT_INO, FileType::Directory, "..".to_string()),
        ];

        for entry in entries {
            let child_path = path.join(&entry.name);
            let mut inodes = self.inodes.lock().unwrap();
            let child_ino = inodes.get_or_create(child_path);
            let kind = if entry.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            full_entries.push((child_ino, kind, entry.name));
        }

        for (i, (ino, kind, name)) in full_entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*ino, (i + 1) as u64, *kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        if self.is_readonly() {
            reply.error(Errno::EACCES);
            return;
        }

        let inodes = self.inodes.lock().unwrap();
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        let child_path = parent_path.join(name);
        match self
            .handle
            .block_on(self.backend.mkdir(&self.dir_id, &child_path))
        {
            Ok(()) => {
                let mut inodes = self.inodes.lock().unwrap();
                let ino = inodes.get_or_create(child_path);
                let now = SystemTime::now();
                let attr = fuser::FileAttr {
                    ino,
                    size: 0,
                    blocks: 0,
                    atime: now,
                    mtime: now,
                    ctime: now,
                    crtime: now,
                    kind: FileType::Directory,
                    perm: 0o755,
                    nlink: 2,
                    uid: unsafe { libc::getuid() },
                    gid: unsafe { libc::getgid() },
                    rdev: 0,
                    blksize: 512,
                    flags: 0,
                };
                reply.entry(&TTL, &attr, Generation(0));
            }
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        if self.is_readonly() {
            reply.error(Errno::EACCES);
            return;
        }

        let inodes = self.inodes.lock().unwrap();
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        let child_path = parent_path.join(name);
        match self
            .handle
            .block_on(self.backend.write_file(&self.dir_id, &child_path, &[]))
        {
            Ok(()) => {
                let mut inodes = self.inodes.lock().unwrap();
                let ino = inodes.get_or_create(child_path);
                let now = SystemTime::now();
                let attr = fuser::FileAttr {
                    ino,
                    size: 0,
                    blocks: 0,
                    atime: now,
                    mtime: now,
                    ctime: now,
                    crtime: now,
                    kind: FileType::RegularFile,
                    perm: 0o644,
                    nlink: 1,
                    uid: unsafe { libc::getuid() },
                    gid: unsafe { libc::getgid() },
                    rdev: 0,
                    blksize: 512,
                    flags: 0,
                };
                reply.created(
                    &TTL,
                    &attr,
                    Generation(0),
                    FileHandle(0),
                    FopenFlags::empty(),
                );
            }
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        if self.is_readonly() {
            reply.error(Errno::EACCES);
            return;
        }

        let inodes = self.inodes.lock().unwrap();
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        let child_path = parent_path.join(name);
        match self
            .handle
            .block_on(self.backend.remove(&self.dir_id, &child_path))
        {
            Ok(()) => reply.ok(),
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        if self.is_readonly() {
            reply.error(Errno::EACCES);
            return;
        }

        let inodes = self.inodes.lock().unwrap();
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        let child_path = parent_path.join(name);
        match self
            .handle
            .block_on(self.backend.remove(&self.dir_id, &child_path))
        {
            Ok(()) => reply.ok(),
            Err(_) => reply.error(Errno::EIO),
        }
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        if self.is_readonly() {
            reply.error(Errno::EACCES);
            return;
        }

        let inodes = self.inodes.lock().unwrap();
        let parent_path = match inodes.get_path(parent) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        let new_parent_path = match inodes.get_path(newparent) {
            Some(p) => p.clone(),
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        drop(inodes);

        let from = parent_path.join(name);
        let to = new_parent_path.join(newname);
        match self
            .handle
            .block_on(self.backend.rename(&self.dir_id, &from, &to))
        {
            Ok(()) => reply.ok(),
            Err(_) => reply.error(Errno::EIO),
        }
    }
}
