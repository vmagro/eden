# perftweaks.py
#
# Copyright 2015 Facebook, Inc.
#
# This software may be used and distributed according to the terms of the
# GNU General Public License version 2 or any later version.
"""extension for tweaking Mercurial features to improve performance."""

from mercurial import branchmap, merge, revlog, scmutil, tags, util
from mercurial.extensions import wrapcommand, wrapfunction
from mercurial.i18n import _
from mercurial.node import nullid, nullrev
import os

testedwith = 'internal'

def extsetup(ui):
    wrapfunction(tags, '_readtagcache', _readtagcache)
    wrapfunction(merge, '_checkcollision', _checkcollision)
    wrapfunction(branchmap.branchcache, 'update', _branchmapupdate)
    if ui.configbool('perftweaks', 'preferdeltas'):
        wrapfunction(revlog.revlog, '_isgooddelta', _isgooddelta)

    wrapfunction(branchmap.branchcache, 'write', _branchmapwrite)
    wrapfunction(branchmap, 'read', _branchmapread)

def _readtagcache(orig, ui, repo):
    """Disables reading tags if the repo is known to not contain any."""
    if ui.configbool('perftweaks', 'disabletags'):
        return (None, None, None, {}, False)

    return orig(ui, repo)

def _checkcollision(orig, repo, wmf, actions):
    """Disables case collision checking since it is known to be very slow."""
    if repo.ui.configbool('perftweaks', 'disablecasecheck'):
        return
    orig(repo, wmf, actions)

def _branchmapupdate(orig, self, repo, revgen):
    if not repo.ui.configbool('perftweaks', 'disablebranchcache'):
        return orig(self, repo, revgen)

    cl = repo.changelog

    # Since we have no branches, the default branch heads are equal to
    # cl.headrevs().
    branchheads = sorted(cl.headrevs())

    self['default'] = [cl.node(rev) for rev in branchheads]
    tiprev = branchheads[-1]
    if tiprev > self.tiprev:
        self.tipnode = cl.node(tiprev)
        self.tiprev = tiprev

    # Copy and paste from branchmap.branchcache.update()
    if not self.validfor(repo):
        # cache key are not valid anymore
        self.tipnode = nullid
        self.tiprev = nullrev
        for heads in self.values():
            tiprev = max(cl.rev(node) for node in heads)
            if tiprev > self.tiprev:
                self.tipnode = cl.node(tiprev)
                self.tiprev = tiprev
    self.filteredhash = scmutil.filteredhash(repo, self.tiprev)
    repo.ui.log('branchcache', 'perftweaks updated %s branch cache\n',
                repo.filtername)

def _branchmapread(orig, repo):
    _preloadrevs(repo)
    return orig(repo)

def _branchmapwrite(orig, self, repo):
    result = orig(self, repo)
    if repo.ui.configbool('perftweaks', 'cachenoderevs', True):
        revs = set()
        nodemap = repo.changelog.nodemap
        for branch, heads in self.iteritems():
            revs.update(nodemap[n] for n in heads)
        name = 'branchheads-%s' % repo.filtername
        _savepreloadrevs(repo, name, revs)

    return result


def _isgooddelta(orig, self, d, textlen):
    """Returns True if the given delta is good. Good means that it is within
    the disk span, disk size, and chain length bounds that we know to be
    performant."""
    if d is None:
        return False

    # - 'dist' is the distance from the base revision -- bounding it limits
    #   the amount of I/O we need to do.
    # - 'compresseddeltalen' is the sum of the total size of deltas we need
    #   to apply -- bounding it limits the amount of CPU we consume.
    dist, l, data, base, chainbase, chainlen, compresseddeltalen = d

    # Our criteria:
    # 1. the delta is not larger than the full text
    # 2. the delta chain cumulative size is not greater than twice the fulltext
    # 3. The chain length is less than the maximum
    #
    # This differs from upstream Mercurial's criteria. They prevent the total
    # ondisk span from chain base to rev from being greater than 4x the full
    # text len. This isn't good enough in our world since if we have 10+
    # branches going on at once, we can easily exceed the 4x limit and cause
    # full texts to be written over and over again.
    if (l > textlen or compresseddeltalen > textlen * 2 or
        (self._maxchainlen and chainlen > self._maxchainlen)):
        return False

    return True

def _cachepath(repo, name):
    return repo.vfs.join('cache', 'noderevs', name)

def _preloadrevs(repo):
    # Preloading the node-rev map for likely to be used revs saves 100ms on
    # every command. This is because normally to look up a node, hg has to scan
    # the changelog.i file backwards, potentially reading through hundreds of
    # thousands of entries and building a cache of them.  Looking up a rev
    # however is fast, because we know exactly what offset in the file to read.
    # Reading old commits is common, since the branchmap needs to to convert old
    # branch heads from node to rev.

    if repo.ui.configbool('perftweaks', 'cachenoderevs', True):
        repo = repo.unfiltered()
        revs = set()
        cachedir = repo.vfs.join('cache', 'noderevs')
        try:
            if os.path.exists(cachedir):
                for cachefile in os.listdir(cachedir):
                    path = _cachepath(repo, cachefile)
                    revs.update(int(r) for r in open(path).readlines())

                getnode = repo.changelog.node
                nodemap = repo.changelog.nodemap
                for r in revs:
                    try:
                        node = getnode(r)
                        nodemap[node] = r
                    except IndexError:
                        # Rev no longer exists
                        pass
        except IOError:
            # No permission to read? No big deal
            pass

def _savepreloadrevs(repo, name, revs):
    if repo.ui.configbool('perftweaks', 'cachenoderevs', True):
        try:
            cachedir = repo.vfs.join('cache', 'noderevs')
            if not os.path.exists(cachedir):
                os.mkdir(cachedir)
            path = _cachepath(repo, name)
            f = util.atomictempfile(path, 'w+')
            f.write('\n'.join(str(r) for r in revs))
            f.close()
        except IOError:
            # No permission to write? No big deal
            pass
