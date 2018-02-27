  $ . "$TESTDIR/library.sh"

  $ cat >> $HGRCPATH <<EOF
  > [treemanifest]
  > sendtrees=True
  > EOF

Setup the server

  $ hginit master
  $ cd master
  $ cat >> .hg/hgrc <<EOF
  > [extensions]
  > bundle2hooks=
  > pushrebase=
  > treemanifest=
  > [treemanifest]
  > server=True
  > [remotefilelog]
  > server=True
  > shallowtrees=True
  > EOF

Make local commits on the server
  $ mkdir subdir
  $ echo x > subdir/x
  $ hg commit -qAm 'add subdir/x'

The following will simulate the transition from flat to tree-only
1. Flat only client, with flat only draft commits
2. Hybrid client, with some flat and some flat+tree draft commits
3. Tree-only client, with only tree commits (old flat are converted)

Create flat manifest client
  $ cd ..
  $ hgcloneshallow ssh://user@dummy/master client -q
  1 files fetched over 1 fetches - (1 misses, 0.00% hit ratio) over * (glob)
  $ cd client
  $ cat >> .hg/hgrc <<EOF
  > [extensions]
  > fbamend=
  > pushrebase=
  > EOF

Make a flat-only draft commit tree
  $ echo f1 >> subdir/x
  $ hg commit -qm 'flat only commit 1 at level 1'
  $ echo f11 >> subdir/x
  $ hg commit -qm 'flat only commit 1 over flat only commit 1 at level 1'
  $ hg up '.^'
  1 files updated, 0 files merged, 0 files removed, 0 files unresolved
  $ echo f12 >> subdir/x
  $ hg commit -qm 'flat only commit 2 over flat only commit 1 at level 1'
  $ echo f121 >> subdir/x
  $ hg commit -qm 'flat only commit 1 over flat only commit 2 at level 2'
  $ hg up '.^^^'
  1 files updated, 0 files merged, 0 files removed, 0 files unresolved

Transition to treeonly client
  $ cat >> .hg/hgrc <<EOF
  > [extensions]
  > treemanifest=
  > fastmanifest=
  > [fastmanifest]
  > usetree=True
  > usecache=False
  > [treemanifest]
  > demanddownload=True
  > treeonly=True
  > EOF

Test working with flat-only draft commits.

- There are no local tree packs.
  $ ls_l .hg/store | grep packs
  [1]

- Viewing flat draft commit would fail when 'treemanifest.demandgenerate' is
False in treeonly mode because there is no tree manifest.

  $ hg log -vpr 'b9b574be2f5d' --config treemanifest.demandgenerate=False \
  > 2>&1 > /dev/null | tail -1
  abort: "unable to find the following nodes locally or on the server: ('', 40f43426c87ba597f0d9553077c72fe06d4e2acb)"

- Viewing a flat draft commit in treeonly mode will generate a tree manifest
for all the commits in the path from the flat draft commit to an ancestor which
has tree manifest. In this case, this implies that tree manifest will be
generated for the commit 'b9b574be2f5d' and its parent commit '9055b56f3916'.

  $ hg log -vpr 'b9b574be2f5d'
  2 trees fetched over * (glob)
  changeset:   2:b9b574be2f5d
  user:        test
  date:        Thu Jan 01 00:00:00 1970 +0000
  files:       subdir/x
  description:
  flat only commit 1 over flat only commit 1 at level 1
  
  
  diff -r 9055b56f3916 -r b9b574be2f5d subdir/x
  --- a/subdir/x	Thu Jan 01 00:00:00 1970 +0000
  +++ b/subdir/x	Thu Jan 01 00:00:00 1970 +0000
  @@ -1,2 +1,3 @@
   x
   f1
  +f11
  
- Now that we have the tree manifest for commit 'b9b574be2f5d', we should be
able to view it even with 'treemanifest.demandgenerate' being False.

  $ hg log -vpr 'b9b574be2f5d' --config treemanifest.demandgenerate=False
  changeset:   2:b9b574be2f5d
  user:        test
  date:        Thu Jan 01 00:00:00 1970 +0000
  files:       subdir/x
  description:
  flat only commit 1 over flat only commit 1 at level 1
  
  
  diff -r 9055b56f3916 -r b9b574be2f5d subdir/x
  --- a/subdir/x	Thu Jan 01 00:00:00 1970 +0000
  +++ b/subdir/x	Thu Jan 01 00:00:00 1970 +0000
  @@ -1,2 +1,3 @@
   x
   f1
  +f11
  
- We should be able to also view the parent of commit 'b9b574be2f5d' i.e. commit
'9055b56f3916' because we now have the tree manifest for it.

  $ hg log -vpr '9055b56f3916' --config treemanifest.demandgenerate=False
  changeset:   1:9055b56f3916
  user:        test
  date:        Thu Jan 01 00:00:00 1970 +0000
  files:       subdir/x
  description:
  flat only commit 1 at level 1
  
  
  diff -r 2278cc8c6ce6 -r 9055b56f3916 subdir/x
  --- a/subdir/x	Thu Jan 01 00:00:00 1970 +0000
  +++ b/subdir/x	Thu Jan 01 00:00:00 1970 +0000
  @@ -1,1 +1,2 @@
   x
  +f1
  
- Check the tree manifest for commit '9055b56f3916' and 'b9b574be2f5d'.

  $ ls_l .hg/store/packs/manifests
  -r--r--r--    1196 028534a0bedee7c3d57bd6fd459f16abe971621b.histidx
  -r--r--r--     183 028534a0bedee7c3d57bd6fd459f16abe971621b.histpack
  -r--r--r--    1106 4bb74ed3582b14a57f34a23c494496a4212af761.dataidx
  -r--r--r--     211 4bb74ed3582b14a57f34a23c494496a4212af761.datapack
  -r--r--r--    1106 5a179c0bb6419ffadbed2c826f2e17b95b05bafb.dataidx
  -r--r--r--     211 5a179c0bb6419ffadbed2c826f2e17b95b05bafb.datapack
  -r--r--r--    1196 9a66f2052faa2af4f56b178a1c2958b52b676046.histidx
  -r--r--r--     183 9a66f2052faa2af4f56b178a1c2958b52b676046.histpack

- Tree manifest data for commit '9055b56f3916'.

  $ hg debugdatapack .hg/store/packs/manifests/5a179c0bb6419ffadbed2c826f2e17b95b05bafb.datapack
  .hg/store/packs/manifests/5a179c0bb6419ffadbed2c826f2e17b95b05bafb:
  subdir:
  Node          Delta Base    Delta Length  Blob Size
  33600a12f793  000000000000  43            (missing)
  
  (empty name):
  Node          Delta Base    Delta Length  Blob Size
  40f43426c87b  000000000000  49            (missing)
  
- Tree manifest data for commit 'b9b574be2f5d'.

  $ hg debugdatapack .hg/store/packs/manifests/4bb74ed3582b14a57f34a23c494496a4212af761.datapack
  .hg/store/packs/manifests/4bb74ed3582b14a57f34a23c494496a4212af761:
  subdir:
  Node          Delta Base    Delta Length  Blob Size
  397e59856f06  000000000000  43            (missing)
  
  (empty name):
  Node          Delta Base    Delta Length  Blob Size
  53c631458e33  000000000000  49            (missing)
  
- Again, this would generate the tree manifest from the corresponding flat
manifest for commit 'f7febcf0f689'.

  $ hg log -vpr 'f7febcf0f689'
  changeset:   3:f7febcf0f689
  parent:      1:9055b56f3916
  user:        test
  date:        Thu Jan 01 00:00:00 1970 +0000
  files:       subdir/x
  description:
  flat only commit 2 over flat only commit 1 at level 1
  
  
  diff -r 9055b56f3916 -r f7febcf0f689 subdir/x
  --- a/subdir/x	Thu Jan 01 00:00:00 1970 +0000
  +++ b/subdir/x	Thu Jan 01 00:00:00 1970 +0000
  @@ -1,2 +1,3 @@
   x
   f1
  +f12
  
  $ ls_l .hg/store/packs/manifests
  -r--r--r--    1196 028534a0bedee7c3d57bd6fd459f16abe971621b.histidx
  -r--r--r--     183 028534a0bedee7c3d57bd6fd459f16abe971621b.histpack
  -r--r--r--    1106 1074860af987f99d7c9e6d053852060e47ed05bd.dataidx
  -r--r--r--     211 1074860af987f99d7c9e6d053852060e47ed05bd.datapack
  -r--r--r--    1106 4bb74ed3582b14a57f34a23c494496a4212af761.dataidx
  -r--r--r--     211 4bb74ed3582b14a57f34a23c494496a4212af761.datapack
  -r--r--r--    1106 5a179c0bb6419ffadbed2c826f2e17b95b05bafb.dataidx
  -r--r--r--     211 5a179c0bb6419ffadbed2c826f2e17b95b05bafb.datapack
  -r--r--r--    1196 5ef2ade5f4492a25bed3204d2e5b6040e5f5f96e.histidx
  -r--r--r--     183 5ef2ade5f4492a25bed3204d2e5b6040e5f5f96e.histpack
  -r--r--r--    1196 9a66f2052faa2af4f56b178a1c2958b52b676046.histidx
  -r--r--r--     183 9a66f2052faa2af4f56b178a1c2958b52b676046.histpack

- Tree manifest data for commit 'f7febcf0f689'.

  $ hg debugdatapack .hg/store/packs/manifests/1074860af987f99d7c9e6d053852060e47ed05bd.datapack
  .hg/store/packs/manifests/1074860af987f99d7c9e6d053852060e47ed05bd:
  subdir:
  Node          Delta Base    Delta Length  Blob Size
  906f17f69284  000000000000  43            (missing)
  
  (empty name):
  Node          Delta Base    Delta Length  Blob Size
  a6875e5fbf69  000000000000  49            (missing)
  
- Clean up generated tree manifests for remaining tests.

  $ rm -rf .hg/store/packs

- Test rebasing of the flat ony commits works as expected.

  $ hg rebase -d '9055b56f3916' -s '3795bd66ca70'
  rebasing 4:3795bd66ca70 "flat only commit 1 over flat only commit 2 at level 2" (tip)
  merging subdir/x
  warning: conflicts while merging subdir/x! (edit, then use 'hg resolve --mark')
  unresolved conflicts (see hg resolve, then hg rebase --continue)
  [1]
