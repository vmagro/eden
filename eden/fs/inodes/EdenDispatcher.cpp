/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

#include "EdenDispatcher.h"

#include <cpptoml.h> // @manual=fbsource//third-party/cpptoml:cpptoml
#include <folly/Format.h>
#include <folly/logging/xlog.h>
#include <gflags/gflags.h>
#include <cstring>
#include <shared_mutex>

#include "eden/fs/config/CheckoutConfig.h"
#include "eden/fs/fuse/DirList.h"
#include "eden/fs/inodes/EdenMount.h"
#include "eden/fs/inodes/FileInode.h"
#include "eden/fs/inodes/InodeMap.h"
#include "eden/fs/inodes/Overlay.h"
#include "eden/fs/inodes/ServerState.h"
#include "eden/fs/inodes/TreeInode.h"
#include "eden/fs/store/ObjectFetchContext.h"
#include "eden/fs/utils/SystemError.h"

using namespace folly;
using std::string;
using std::vector;

DEFINE_int32(
    inode_reserve,
    1000000,
    "pre-size inode hash table for this many entries");

namespace facebook {
namespace eden {

#ifndef _WIN32
EdenDispatcher::EdenDispatcher(EdenMount* mount)
    : Dispatcher(mount->getStats()),
      mount_(mount),
      inodeMap_(mount_->getInodeMap()) {}

namespace {

/** Compute a fuse_entry_out */
fuse_entry_out computeEntryParam(const Dispatcher::Attr& attr) {
  DCHECK(attr.st.st_ino) << "We should never return a 0 inode to FUSE";
  fuse_entry_out entry = {};
  entry.nodeid = attr.st.st_ino;
  entry.generation = 0;
  auto fuse_attr = attr.asFuseAttr();
  entry.attr = fuse_attr.attr;
  entry.attr_valid = fuse_attr.attr_valid;
  entry.attr_valid_nsec = fuse_attr.attr_valid_nsec;
  entry.entry_valid = fuse_attr.attr_valid;
  entry.entry_valid_nsec = fuse_attr.attr_valid_nsec;
  return entry;
}

constexpr int64_t kBrokenInodeCacheSeconds = 5;

Dispatcher::Attr attrForInodeWithCorruptOverlay(InodeNumber ino) noexcept {
  struct stat st = {};
  st.st_ino = ino.get();
  st.st_mode = S_IFREG;
  return Dispatcher::Attr{st, kBrokenInodeCacheSeconds};
}
} // namespace

folly::Future<Dispatcher::Attr> EdenDispatcher::getattr(
    InodeNumber ino,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "getattr({})", ino);
  return inodeMap_->lookupInode(ino)
      .thenValue(
          [&context](const InodePtr& inode) { return inode->stat(context); })
      .thenValue([](const struct stat& st) { return Dispatcher::Attr{st}; });
}

folly::Future<uint64_t> EdenDispatcher::opendir(InodeNumber ino, int flags) {
  FB_LOGF(
      mount_->getStraceLogger(), DBG7, "opendir({}, flags={:x})", ino, flags);
#ifdef FUSE_NO_OPENDIR_SUPPORT
  if (getConnInfo().flags & FUSE_NO_OPENDIR_SUPPORT) {
    // If the kernel understands FUSE_NO_OPENDIR_SUPPORT, then returning ENOSYS
    // means that no further opendir() nor releasedir() calls will make it into
    // Eden.
    folly::throwSystemErrorExplicit(
        ENOSYS, "Eden opendir() calls are stateless and not required");
  }
#endif
  return 0;
}

folly::Future<folly::Unit> EdenDispatcher::releasedir(
    InodeNumber ino,
    uint64_t fh) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "releasedir({}, {})", ino, fh);
  return folly::unit;
}

folly::Future<fuse_entry_out> EdenDispatcher::lookup(
    uint64_t /*requestID*/,
    InodeNumber parent,
    PathComponentPiece namepiece,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "lookup({}, {})", parent, namepiece);
  return inodeMap_->lookupTreeInode(parent)
      .thenValue([name = PathComponent(namepiece),
                  &context](const TreeInodePtr& tree) {
        return tree->getOrLoadChild(name, context);
      })
      .thenValue([&context](const InodePtr& inode) {
        return folly::makeFutureWith([&]() { return inode->stat(context); })
            .thenTry([inode](folly::Try<struct stat> maybeStat) {
              if (maybeStat.hasValue()) {
                inode->incFuseRefcount();
                return computeEntryParam(Dispatcher::Attr{maybeStat.value()});
              } else {
                // The most common case for stat() failing is if this file is
                // materialized but the data for it in the overlay is missing
                // or corrupt.  This can happen after a hard reboot where the
                // overlay data was not synced to disk first.
                //
                // We intentionally want to return a result here rather than
                // failing; otherwise we can't return the inode number to the
                // kernel at all.  This blocks other operations on the file,
                // like FUSE_UNLINK.  By successfully returning from the
                // lookup we allow clients to remove this corrupt file with an
                // unlink operation.  (Even though FUSE_UNLINK does not require
                // the child inode number, the kernel does not appear to send a
                // FUSE_UNLINK request to us if it could not get the child inode
                // number first.)
                XLOG(WARN) << "error getting attributes for inode "
                           << inode->getNodeId() << " (" << inode->getLogPath()
                           << "): " << maybeStat.exception().what();
                inode->incFuseRefcount();
                return computeEntryParam(
                    attrForInodeWithCorruptOverlay(inode->getNodeId()));
              }
            });
      })
      .thenError(
          folly::tag_t<std::system_error>{}, [](const std::system_error& err) {
            // Translate ENOENT into a successful response with an
            // inode number of 0 and a large entry_valid time, to let the kernel
            // cache this negative lookup result.
            if (isEnoent(err)) {
              fuse_entry_out entry = {};
              entry.attr_valid =
                  std::numeric_limits<decltype(entry.attr_valid)>::max();
              entry.entry_valid =
                  std::numeric_limits<decltype(entry.entry_valid)>::max();
              return entry;
            }
            throw err;
          });
}

folly::Future<Dispatcher::Attr> EdenDispatcher::setattr(
    InodeNumber ino,
    const fuse_setattr_in& attr) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "setattr({})", ino);

  // Even though mounts are created with the nosuid flag, explicitly disallow
  // setting suid, sgid, and sticky bits on any inodes. This lets us avoid
  // explicitly clearing these bits on writes() which is required for correct
  // behavior under FUSE_HANDLE_KILLPRIV.
  if ((attr.valid & FATTR_MODE) &&
      (attr.mode & (S_ISUID | S_ISGID | S_ISVTX))) {
    folly::throwSystemErrorExplicit(EPERM, "Extra mode bits are disallowed");
  }

  return inodeMap_->lookupInode(ino).thenValue(
      [attr](const InodePtr& inode) { return inode->setattr(attr); });
}

void EdenDispatcher::forget(InodeNumber ino, unsigned long nlookup) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "forget({}, {})", ino, nlookup);
  inodeMap_->decFuseRefcount(ino, nlookup);
}

folly::Future<uint64_t> EdenDispatcher::open(InodeNumber ino, int flags) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "open({}, flags={:x})", ino, flags);
#ifdef FUSE_NO_OPEN_SUPPORT
  if (getConnInfo().flags & FUSE_NO_OPEN_SUPPORT) {
    // If the kernel understands FUSE_NO_OPEN_SUPPORT, then returning ENOSYS
    // means that no further open() nor release() calls will make it into Eden.
    folly::throwSystemErrorExplicit(
        ENOSYS, "Eden open() calls are stateless and not required");
  }
#endif
  return 0;
}

folly::Future<fuse_entry_out> EdenDispatcher::create(
    InodeNumber parent,
    PathComponentPiece name,
    mode_t mode,
    int flags) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "create({}, {}, {:#x}, {:#x})",
      parent,
      name,
      mode,
      flags);
  // force 'mode' to be regular file, in which case rdev arg to mknod is ignored
  // (and thus can be zero)
  mode = S_IFREG | (07777 & mode);
  return inodeMap_->lookupTreeInode(parent).thenValue(
      [=](const TreeInodePtr& inode) {
        auto childName = PathComponent{name};
        auto child = inode->mknod(childName, mode, 0, InvalidationRequired::No);
        static auto context = ObjectFetchContext::getNullContextWithCauseDetail(
            "EdenDispatcher::create");
        return child->stat(*context).thenValue(
            [child](struct stat st) -> fuse_entry_out {
              child->incFuseRefcount();
              return computeEntryParam(Dispatcher::Attr{st});
            });
      });
}

folly::Future<BufVec> EdenDispatcher::read(
    InodeNumber ino,
    size_t size,
    off_t off,
    ObjectFetchContext& context) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "read({}, off={}, len={})",
      ino,
      off,
      size);
  return inodeMap_->lookupFileInode(ino).thenValue(
      [&context, size, off](FileInodePtr&& inode) {
        return inode->read(size, off, context);
      });
}

folly::Future<size_t>
EdenDispatcher::write(InodeNumber ino, folly::StringPiece data, off_t off) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "write({}, off={}, len={})",
      ino,
      off,
      data.size());
  return inodeMap_->lookupFileInode(ino).thenValue(
      [copy = data.str(), off](FileInodePtr&& inode) {
        return inode->write(copy, off);
      });
}

Future<Unit> EdenDispatcher::flush(
    InodeNumber /* ino */,
    uint64_t /* lock_owner */) {
  // Return ENOSYS from flush.
  // This will cause the kernel to stop sending future flush() calls.
  return makeFuture<Unit>(makeSystemErrorExplicit(ENOSYS, "flush"));
}

folly::Future<folly::Unit> EdenDispatcher::fsync(
    InodeNumber ino,
    bool datasync) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "fsync({})", ino);
  return inodeMap_->lookupFileInode(ino).thenValue(
      [datasync](FileInodePtr inode) { return inode->fsync(datasync); });
}

Future<Unit> EdenDispatcher::fsyncdir(
    InodeNumber /* ino */,
    bool /* datasync */) {
  // Return ENOSYS from fsyncdir. The kernel will stop sending them.
  //
  // In a possible future where the tree structure is stored in a SQLite
  // database, we could handle this request by waiting for SQLite's
  // write-ahead-log to be flushed.
  return makeFuture<Unit>(makeSystemErrorExplicit(ENOSYS, "fsyncdir"));
}

folly::Future<std::string> EdenDispatcher::readlink(
    InodeNumber ino,
    bool kernelCachesReadlink) {
  static auto context = ObjectFetchContext::getNullContextWithCauseDetail(
      "EdenDispatcher::readlink");
  FB_LOGF(mount_->getStraceLogger(), DBG7, "readlink({})", ino);
  return inodeMap_->lookupFileInode(ino).thenValue(
      [kernelCachesReadlink](const FileInodePtr& inode) {
        // Only release the symlink blob after it's loaded if we can assume the
        // FUSE will cache the result in the kernel's page cache.
        return inode->readlink(
            *context,
            kernelCachesReadlink ? CacheHint::NotNeededAgain
                                 : CacheHint::LikelyNeededAgain);
      });
}

folly::Future<DirList> EdenDispatcher::readdir(
    InodeNumber ino,
    DirList&& dirList,
    off_t offset,
    uint64_t /*fh*/,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "readdir({}, {})", ino, offset);
  return inodeMap_->lookupTreeInode(ino).thenValue(
      [dirList = std::move(dirList), offset, &context](
          TreeInodePtr inode) mutable {
        return inode->readdir(std::move(dirList), offset, context);
      });
}

folly::Future<fuse_entry_out> EdenDispatcher::mknod(
    InodeNumber parent,
    PathComponentPiece name,
    mode_t mode,
    dev_t rdev) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "mknod({}, {}, {:#x}, {:#x})",
      parent,
      name,
      mode,
      rdev);
  static auto context = ObjectFetchContext::getNullContextWithCauseDetail(
      "EdenDispatcher::mknod");
  return inodeMap_->lookupTreeInode(parent).thenValue(
      [childName = PathComponent{name}, mode, rdev](const TreeInodePtr& inode) {
        auto child =
            inode->mknod(childName, mode, rdev, InvalidationRequired::No);
        return child->stat(*context).thenValue(
            [child](struct stat st) -> fuse_entry_out {
              child->incFuseRefcount();
              return computeEntryParam(Dispatcher::Attr{st});
            });
      });
}

folly::Future<fuse_entry_out> EdenDispatcher::mkdir(
    InodeNumber parent,
    PathComponentPiece name,
    mode_t mode) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "mkdir({}, {}, {:#x})",
      parent,
      name,
      mode);
  static auto context = ObjectFetchContext::getNullContextWithCauseDetail(
      "EdenDispatcher::mkdir");
  return inodeMap_->lookupTreeInode(parent).thenValue(
      [childName = PathComponent{name}, mode](const TreeInodePtr& inode) {
        auto child = inode->mkdir(childName, mode, InvalidationRequired::No);
        return child->stat(*context).thenValue([child](struct stat st) {
          child->incFuseRefcount();
          return computeEntryParam(Dispatcher::Attr{st});
        });
      });
}

folly::Future<folly::Unit> EdenDispatcher::unlink(
    InodeNumber parent,
    PathComponentPiece name) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "unlink({}, {})", parent, name);
  return inodeMap_->lookupTreeInode(parent).thenValue(
      [childName = PathComponent{name}](const TreeInodePtr& inode) {
        // No need to flush the kernel cache because FUSE will do that for us.
        return inode->unlink(childName, InvalidationRequired::No);
      });
}

folly::Future<folly::Unit> EdenDispatcher::rmdir(
    InodeNumber parent,
    PathComponentPiece name) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "rmdir({}, {})", parent, name);
  return inodeMap_->lookupTreeInode(parent).thenValue(
      [childName = PathComponent{name}](const TreeInodePtr& inode) {
        // No need to flush the kernel cache because FUSE will do that for us.
        return inode->rmdir(childName, InvalidationRequired::No);
      });
}

folly::Future<fuse_entry_out> EdenDispatcher::symlink(
    InodeNumber parent,
    PathComponentPiece name,
    StringPiece link) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "symlink({}, {}, {})",
      parent,
      name,
      link);
  static auto context = ObjectFetchContext::getNullContextWithCauseDetail(
      "EdenDispatcher::symlink");
  return inodeMap_->lookupTreeInode(parent).thenValue(
      [linkContents = link.str(),
       childName = PathComponent{name}](const TreeInodePtr& inode) {
        auto symlinkInode =
            inode->symlink(childName, linkContents, InvalidationRequired::No);
        symlinkInode->incFuseRefcount();
        return symlinkInode->stat(*context).thenValue(
            [symlinkInode](struct stat st) {
              return computeEntryParam(Dispatcher::Attr{st});
            });
      });
}

folly::Future<folly::Unit> EdenDispatcher::rename(
    InodeNumber parent,
    PathComponentPiece namePiece,
    InodeNumber newParent,
    PathComponentPiece newNamePiece) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "rename({}, {}, {}, {})",
      parent,
      namePiece,
      newParent,
      newNamePiece);
  // Start looking up both parents
  auto parentFuture = inodeMap_->lookupTreeInode(parent);
  auto newParentFuture = inodeMap_->lookupTreeInode(newParent);
  // Do the rename once we have looked up both parents.
  return std::move(parentFuture)
      .thenValue([npFuture = std::move(newParentFuture),
                  name = PathComponent{namePiece},
                  newName = PathComponent{newNamePiece}](
                     const TreeInodePtr& parent) mutable {
        return std::move(npFuture).thenValue(
            [parent, name, newName](const TreeInodePtr& newParent) {
              return parent->rename(
                  name, newParent, newName, InvalidationRequired::No);
            });
      });
}

folly::Future<fuse_entry_out> EdenDispatcher::link(
    InodeNumber ino,
    InodeNumber newParent,
    PathComponentPiece newName) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "link({}, {}, {})",
      ino,
      newParent,
      newName);

  validatePathComponentLength(newName);

  // We intentionally do not support hard links.
  // These generally cannot be tracked in source control (git or mercurial)
  // and are not portable to non-Unix platforms.
  folly::throwSystemErrorExplicit(
      EPERM, "hard links are not supported in eden mount points");
}

Future<string> EdenDispatcher::getxattr(InodeNumber ino, StringPiece name) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "getxattr({}, {})", ino, name);
  return inodeMap_->lookupInode(ino).thenValue(
      [attrName = name.str()](const InodePtr& inode) {
        return inode->getxattr(attrName);
      });
}

Future<vector<string>> EdenDispatcher::listxattr(InodeNumber ino) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "listxattr({})", ino);
  return inodeMap_->lookupInode(ino).thenValue(
      [](const InodePtr& inode) { return inode->listxattr(); });
}

folly::Future<struct fuse_kstatfs> EdenDispatcher::statfs(InodeNumber /*ino*/) {
  struct fuse_kstatfs info = {};

  // Pass through the overlay free space stats; this gives a more reasonable
  // estimation of available storage space than the zeroes that we'd report
  // otherwise.  This is important because eg: Finder on macOS inspects disk
  // space prior to initiating a copy and will refuse to start a copy if
  // the disk appears to be full.
  auto overlayStats = mount_->getOverlay()->statFs();
  info.blocks = overlayStats.f_blocks;
  info.bfree = overlayStats.f_bfree;
  info.bavail = overlayStats.f_bavail;
  info.files = overlayStats.f_files;
  info.ffree = overlayStats.f_ffree;

  // Suggest a large blocksize to software that looks at that kind of thing
  // bsize will be returned to applications that call pathconf() with
  // _PC_REC_MIN_XFER_SIZE
  info.bsize = getConnInfo().max_readahead;

  // The fragment size is returned as the _PC_REC_XFER_ALIGN and
  // _PC_ALLOC_SIZE_MIN pathconf() settings.
  // 4096 is commonly used by many filesystem types.
  info.frsize = 4096;

  // Ensure that namelen is set to a non-zero value.
  // The value we return here will be visible to programs that call pathconf()
  // with _PC_NAME_MAX.  Returning 0 will confuse programs that try to honor
  // this value.
  info.namelen = 255;

  return info;
}
#else
namespace {

const RelativePath kDotEdenConfigPath{".eden/config"};
const std::string kConfigRootPath{"root"};
const std::string kConfigSocketPath{"socket"};
const std::string kConfigClientPath{"client"};
const std::string kConfigTable{"Config"};

std::string makeDotEdenConfig(EdenMount& mount) {
  auto repoPath = mount.getPath();
  auto socketPath = mount.getServerState()->getSocketPath();
  auto clientPath = mount.getConfig()->getClientDirectory();

  auto rootTable = cpptoml::make_table();
  auto configTable = cpptoml::make_table();
  configTable->insert(kConfigRootPath, repoPath.c_str());
  configTable->insert(kConfigSocketPath, socketPath.c_str());
  configTable->insert(kConfigClientPath, clientPath.c_str());
  rootTable->insert(kConfigTable, configTable);

  std::ostringstream stream;
  stream << *rootTable;
  return stream.str();
}

} // namespace

EdenDispatcher::EdenDispatcher(EdenMount* mount)
    : Dispatcher(mount->getStats()),
      mount_{mount},
      dotEdenConfig_{makeDotEdenConfig(*mount)} {}

folly::Future<folly::Unit> EdenDispatcher::opendir(
    RelativePathPiece path,
    const Guid guid,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "opendir({}, guid={})", path, guid);

  return mount_->getInode(path, context)
      .thenValue([&context](const InodePtr inode) {
        auto treePtr = inode.asTreePtr();
        return treePtr->readdir(context);
      })
      .thenValue([this, guid = std::move(guid)](auto&& dirents) {
        auto [iterator, inserted] =
            enumSessions_.wlock()->emplace(guid, std::move(dirents));
        DCHECK(inserted);

        return folly::unit;
      });
}

void EdenDispatcher::closedir(const Guid& guid) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "closedir({})", guid);

  auto erasedCount = enumSessions_.wlock()->erase(guid);
  DCHECK(erasedCount == 1);
}

HRESULT EdenDispatcher::getEnumerationData(
    const PRJ_CALLBACK_DATA& callbackData,
    const GUID& enumerationId,
    PCWSTR searchExpression,
    PRJ_DIR_ENTRY_BUFFER_HANDLE bufferHandle) noexcept {
  try {
    auto guid = Guid(enumerationId);
    FB_LOGF(
        mount_->getStraceLogger(),
        DBG7,
        "readdir({}, searchExpression={})",
        guid,
        searchExpression == nullptr
            ? "<nullptr>"
            : wideToMultibyteString<std::string>(searchExpression));

    auto lockedSessions = enumSessions_.rlock();
    auto sessionIterator = lockedSessions->find(guid);
    if (sessionIterator == lockedSessions->end()) {
      XLOG(DBG5) << "Enum instance not found: "
                 << RelativePath(callbackData.FilePathName);
      return HRESULT_FROM_WIN32(ERROR_INVALID_PARAMETER);
    }

    auto shouldRestart =
        bool(callbackData.Flags & PRJ_CB_DATA_FLAG_ENUM_RESTART_SCAN);

    // We won't ever get concurrent callbacks for a given enumeration, it is
    // therefore safe to modify the session here even though we do not hold an
    // exclusive lock to it.
    auto& session = const_cast<Enumerator&>(sessionIterator->second);

    if (session.isSearchExpressionEmpty() || shouldRestart) {
      if (searchExpression != nullptr) {
        session.saveExpression(searchExpression);
      } else {
        session.saveExpression(L"*");
      }
    }

    if (shouldRestart) {
      session.restart();
    }

    //
    // Traverse the list enumeration list and fill the remaining entry. Start
    // from where the last call left off.
    //
    for (const FileMetadata* entry; (entry = session.current());
         session.advance()) {
      auto fileInfo = PRJ_FILE_BASIC_INFO();

      fileInfo.IsDirectory = entry->isDirectory;
      fileInfo.FileSize = entry->size;

      XLOGF(
          DBG6,
          "Enum {} {} size= {}",
          PathComponent(entry->name),
          fileInfo.IsDirectory ? "Dir" : "File",
          fileInfo.FileSize);

      if (S_OK !=
          PrjFillDirEntryBuffer(entry->name.c_str(), &fileInfo, bufferHandle)) {
        // We are out of buffer space. This entry didn't make it. Return without
        // increment.
        return S_OK;
      }
    }
    return S_OK;
  } catch (const std::exception& ex) {
    return exceptionToHResult(ex);
  }
}

folly::Future<std::optional<InodeMetadata>> EdenDispatcher::lookup(
    RelativePath path,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "lookup({})", path);

  return mount_->getInode(path, context)
      .thenValue(
          [&context](const InodePtr inode) mutable
          -> folly::Future<std::optional<InodeMetadata>> {
            return inode->stat(context).thenValue(
                [inode = std::move(inode)](struct stat&& stat) {
                  // Ensure that the OS has a record of the canonical
                  // file name, and not just whatever case was used to
                  // lookup the file
                  size_t size = stat.st_size;
                  return InodeMetadata{*inode->getPath(), size, inode->isDir()};
                });
          })
      .thenError(
          folly::tag_t<std::system_error>{},
          [path = std::move(path), this](const std::system_error& ex)
              -> folly::Future<std::optional<InodeMetadata>> {
            if (isEnoent(ex)) {
              if (path == kDotEdenConfigPath) {
                return folly::makeFuture(InodeMetadata{
                    std::move(path), dotEdenConfig_.length(), false});
              } else {
                XLOG(DBG6) << path << ": File not found";
                return folly::makeFuture(std::nullopt);
              }
            }
            return folly::makeFuture<std::optional<InodeMetadata>>(ex);
          });
}

folly::Future<bool> EdenDispatcher::access(
    RelativePath path,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "access({})", path);

  return mount_->getInode(path, context)
      .thenValue([](const InodePtr) { return true; })
      .thenError(
          folly::tag_t<std::system_error>{},
          [path = std::move(path)](const std::system_error& ex) {
            if (isEnoent(ex)) {
              if (path == kDotEdenConfigPath) {
                return folly::makeFuture(true);
              }
              return folly::makeFuture(false);
            }
            return folly::makeFuture<bool>(ex);
          });
}

folly::Future<std::string> EdenDispatcher::read(
    RelativePath path,
    uint64_t byteOffset,
    uint32_t length,
    ObjectFetchContext& context) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "read({}, off={}, len={})",
      path,
      byteOffset,
      length);

  return mount_->getInode(path, context)
      .thenValue([&context](const InodePtr inode) {
        auto fileInode = inode.asFilePtr();
        return fileInode->readAll(context);
      })
      .thenError(
          folly::tag_t<std::system_error>{},
          [path = std::move(path), this](const std::system_error& ex) {
            if (isEnoent(ex) && path == kDotEdenConfigPath) {
              return folly::makeFuture<std::string>(
                  std::string(dotEdenConfig_));
            }
            return folly::makeFuture<std::string>(ex);
          });
}

namespace {
folly::Future<TreeInodePtr> createDirInode(
    const EdenMount& mount,
    const RelativePathPiece path,
    ObjectFetchContext& context) {
  return mount.getInode(path, context)
      .thenValue([](const InodePtr inode) { return inode.asTreePtr(); })
      .thenError(
          folly::tag_t<std::system_error>{},
          [=, &mount](const std::system_error& ex) {
            if (!isEnoent(ex)) {
              return folly::makeFuture<TreeInodePtr>(ex);
            }

            mount.getStats()
                ->getChannelStatsForCurrentThread()
                .outOfOrderCreate.addValue(1);
            XLOG(DBG2) << "Out of order directory creation notification for: "
                       << path;

            /*
             * ProjectedFS notifications are asynchronous and sent after the
             * fact. This means that we can get a notification on a
             * file/directory before the parent directory notification has been
             * completed. This should be a very rare event and thus the code
             * below is pessimistic and will try to create all parent
             * directories.
             */

            auto fut = folly::makeFuture(mount.getRootInode());
            for (auto parent : path.paths()) {
              fut = std::move(fut).thenValue([parent](TreeInodePtr treeInode) {
                try {
                  treeInode->mkdir(
                      parent.basename(), _S_IFDIR, InvalidationRequired::No);
                } catch (std::system_error& ex) {
                  if (ex.code().value() != EEXIST) {
                    throw;
                  }
                }

                return treeInode->getOrLoadChildTree(parent.basename());
              });
            }

            return fut;
          });
}

folly::Future<folly::Unit> createFile(
    const EdenMount& mount,
    const RelativePathPiece path,
    bool isDirectory,
    ObjectFetchContext& context) {
  return createDirInode(mount, path.dirname(), context)
      .thenValue([=, &mount](const TreeInodePtr treeInode) {
        if (isDirectory) {
          try {
            treeInode->mkdir(
                path.basename(), _S_IFDIR, InvalidationRequired::No);
          } catch (std::system_error& ex) {
            /*
             * If a concurrent createFile for a child of this directory finished
             * before this one, the directory will already exist. This is not an
             * error.
             */
            if (ex.code().value() != EEXIST) {
              return folly::makeFuture<folly::Unit>(ex);
            }
          }
        } else {
          treeInode->mknod(
              path.basename(), _S_IFREG, 0, InvalidationRequired::No);
        }

        return folly::makeFuture(folly::unit);
      });
}

folly::Future<folly::Unit> materializeFile(
    const EdenMount& mount,
    const RelativePathPiece path,
    ObjectFetchContext& context) {
  return mount.getInode(path, context).thenValue([](const InodePtr inode) {
    auto fileInode = inode.asFilePtr();
    fileInode->materialize();
    return folly::unit;
  });
}

folly::Future<folly::Unit> renameFile(
    const EdenMount& mount,
    const RelativePathPiece oldPath,
    const RelativePathPiece newPath,
    ObjectFetchContext& context) {
  auto oldParentInode = createDirInode(mount, oldPath.dirname(), context);
  auto newParentInode = createDirInode(mount, newPath.dirname(), context);

  return std::move(oldParentInode)
      .thenValue([=, newParentInode = std::move(newParentInode)](
                     const TreeInodePtr oldParentTreePtr) mutable {
        return std::move(newParentInode)
            .thenValue([=, oldParentTreePtr = std::move(oldParentTreePtr)](
                           const TreeInodePtr newParentTreePtr) {
              // TODO(xavierd): In the case where the oldPath is actually being
              // created in another thread, EdenFS simply might not know about
              // it at this point. Creating the file and renaming it at this
              // point won't help as the other thread will re-create it. In the
              // future, we may want to try, wait a bit and retry, or re-think
              // this and somehow order requests so the file creation always
              // happens before the rename.
              //
              // This should be *extremely* rare, for now let's just let it
              // error out.
              return oldParentTreePtr->rename(
                  oldPath.basename(),
                  newParentTreePtr,
                  newPath.basename(),
                  InvalidationRequired::No);
            });
      });
}

folly::Future<folly::Unit> removeFile(
    const EdenMount& mount,
    const RelativePathPiece path,
    bool isDirectory,
    ObjectFetchContext& context) {
  return mount.getInode(path.dirname(), context)
      .thenValue([=](const InodePtr inode) {
        auto treeInodePtr = inode.asTreePtr();
        if (isDirectory) {
          return treeInodePtr->rmdir(path.basename(), InvalidationRequired::No);
        } else {
          return treeInodePtr->unlink(
              path.basename(), InvalidationRequired::No);
        }
      });
}

} // namespace

folly::Future<folly::Unit> EdenDispatcher::newFileCreated(
    RelativePathPiece relPath,
    RelativePathPiece destPath,
    bool isDirectory,
    ObjectFetchContext& context) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "{}({})",
      isDirectory ? "mkdir" : "mknod",
      relPath);
  return createFile(*mount_, relPath, isDirectory, context);
}

folly::Future<folly::Unit> EdenDispatcher::fileOverwritten(
    RelativePathPiece relPath,
    RelativePathPiece destPath,
    bool isDirectory,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "overwrite({})", relPath);
  return materializeFile(*mount_, relPath, context);
}

folly::Future<folly::Unit> EdenDispatcher::fileHandleClosedFileModified(
    RelativePathPiece relPath,
    RelativePathPiece destPath,
    bool isDirectory,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "modified({})", relPath);
  return materializeFile(*mount_, relPath, context);
}

folly::Future<folly::Unit> EdenDispatcher::fileRenamed(
    RelativePathPiece oldPath,
    RelativePathPiece newPath,
    bool isDirectory,
    ObjectFetchContext& context) {
  FB_LOGF(
      mount_->getStraceLogger(), DBG7, "rename({} -> {})", oldPath, newPath);

  // When files are moved in and out of the repo, the rename paths are
  // empty, handle these like creation/removal of files.
  if (oldPath.empty()) {
    return createFile(*mount_, newPath, isDirectory, context);
  } else if (newPath.empty()) {
    return removeFile(*mount_, oldPath, isDirectory, context);
  } else {
    return renameFile(*mount_, oldPath, newPath, context);
  }
}

folly::Future<folly::Unit> EdenDispatcher::preRename(
    RelativePathPiece oldPath,
    RelativePathPiece newPath,
    bool isDirectory,
    ObjectFetchContext& context) {
  FB_LOGF(
      mount_->getStraceLogger(), DBG7, "prerename({} -> {})", oldPath, newPath);
  return folly::unit;
}

folly::Future<folly::Unit> EdenDispatcher::fileHandleClosedFileDeleted(
    RelativePathPiece oldPath,
    RelativePathPiece destPath,
    bool isDirectory,
    ObjectFetchContext& context) {
  FB_LOGF(
      mount_->getStraceLogger(),
      DBG7,
      "{}({})",
      isDirectory ? "rmdir" : "unlink",
      oldPath);
  return removeFile(*mount_, oldPath, isDirectory, context);
}

folly::Future<folly::Unit> EdenDispatcher::preSetHardlink(
    RelativePathPiece relPath,
    RelativePathPiece destPath,
    bool isDirectory,
    ObjectFetchContext& context) {
  FB_LOGF(mount_->getStraceLogger(), DBG7, "link({})", relPath);
  return folly::makeFuture<folly::Unit>(makeHResultErrorExplicit(
      HRESULT_FROM_WIN32(ERROR_ACCESS_DENIED),
      fmt::format(FMT_STRING("Hardlinks are not supported: {}"), relPath)));
}
#endif

} // namespace eden
} // namespace facebook
