  $ enable amend rebase undo
  $ setconfig experimental.evolution=
  $ setconfig visibility.tracking=on
  $ setconfig mutation.record=true mutation.enabled=true mutation.date="0 0"
  $ setconfig hint.ack=undo
  $ cat >> $HGRCPATH <<EOF
  > [templatealias]
  > sl_mutation_names = dict(amend="Amended as", rebase="Rebased to", split="Split into", fold="Folded into", histedit="Histedited to", rewrite="Rewritten into", land="Landed as")
  > sl_mutations = "{join(mutations % '({get(sl_mutation_names, operation, "Rewritten using {operation} into")} {join(successors % "{node|short}", ", ")})', ' ')}"
  > EOF

Useful functions
  $ mkcommit()
  > {
  >   echo "$1" > "$1"
  >   hg add "$1"
  >   hg commit -m "$1"
  > }

  $ tglogm()
  > {
  >   hg log -G -T "{rev}: {node|short} '{desc}' {bookmarks} {sl_mutations}" "$@"
  > }

Setup
  $ newrepo
  $ mkcommit root
  $ mkcommit public1
  $ mkcommit public2
  $ hg phase -p .

Simple creation and amending of draft commits

  $ mkcommit draft1
  $ sort < .hg/store/visibleheads
  ca9d66205acae45570c29bea55877bb8031aa453
  v1
  $ hg amend -m "draft1 amend1"
  $ sort < .hg/store/visibleheads
  bc066ca12b451d14668c7a3e38757449b7d6a104
  v1
  $ mkcommit draft2
  $ tglogp --hidden
  @  5: 467d8aa13aef draft 'draft2'
  |
  o  4: bc066ca12b45 draft 'draft1 amend1'
  |
  | x  3: ca9d66205aca draft 'draft1'
  |/
  o  2: 4f416a252ac8 public 'public2'
  |
  o  1: 175dbab47dcc public 'public1'
  |
  o  0: 1e4be0697311 public 'root'
  
  $ sort < .hg/store/visibleheads
  467d8aa13aef105d18160ea682d5cf20d8941d06
  v1

  $ hg debugstrip -r . --config amend.safestrip=False
  0 files updated, 0 files merged, 1 files removed, 0 files unresolved
  saved backup bundle to $TESTTMP/* (glob)
  $ tglogp --hidden
  @  4: bc066ca12b45 draft 'draft1 amend1'
  |
  | x  3: ca9d66205aca draft 'draft1'
  |/
  o  2: 4f416a252ac8 public 'public2'
  |
  o  1: 175dbab47dcc public 'public1'
  |
  o  0: 1e4be0697311 public 'root'
  
  $ sort < .hg/store/visibleheads
  bc066ca12b451d14668c7a3e38757449b7d6a104
  v1

  $ mkcommit draft2a
  $ hg rebase -s ".^" -d 1
  rebasing 4:bc066ca12b45 "draft1 amend1"
  rebasing 5:2ccd7cddaa94 "draft2a" (tip)
  $ tglogp
  @  7: ecfc0c412bb8 draft 'draft2a'
  |
  o  6: 96b7359a7ee5 draft 'draft1 amend1'
  |
  | o  2: 4f416a252ac8 public 'public2'
  |/
  o  1: 175dbab47dcc public 'public1'
  |
  o  0: 1e4be0697311 public 'root'
  
  $ sort < .hg/store/visibleheads
  ecfc0c412bb878c3e7b1b3468cae773b473fd3ec
  v1
  $ hg rebase -s . -d 2
  rebasing 7:ecfc0c412bb8 "draft2a" (tip)
  $ tglogp
  @  8: af54c09bb37d draft 'draft2a'
  |
  | o  6: 96b7359a7ee5 draft 'draft1 amend1'
  | |
  o |  2: 4f416a252ac8 public 'public2'
  |/
  o  1: 175dbab47dcc public 'public1'
  |
  o  0: 1e4be0697311 public 'root'
  
  $ sort < .hg/store/visibleheads
  96b7359a7ee5350b94be6e5c5dd480751a031498
  af54c09bb37da36975b8d482f660f62f95697a35
  v1

Simple phase adjustments

  $ hg phase -p 6
  $ sort < .hg/store/visibleheads
  af54c09bb37da36975b8d482f660f62f95697a35
  v1
  $ hg phase -df 6
  $ sort < .hg/store/visibleheads
  96b7359a7ee5350b94be6e5c5dd480751a031498
  af54c09bb37da36975b8d482f660f62f95697a35
  v1

  $ mkcommit draft3
  $ mkcommit draft4
  $ tglogp
  @  10: f3f5679a1c9c draft 'draft4'
  |
  o  9: 5dabc7b08ef9 draft 'draft3'
  |
  o  8: af54c09bb37d draft 'draft2a'
  |
  | o  6: 96b7359a7ee5 draft 'draft1 amend1'
  | |
  o |  2: 4f416a252ac8 public 'public2'
  |/
  o  1: 175dbab47dcc public 'public1'
  |
  o  0: 1e4be0697311 public 'root'
  
  $ sort < .hg/store/visibleheads
  96b7359a7ee5350b94be6e5c5dd480751a031498
  f3f5679a1c9cb5a79334a3bbb87b359864c44ce4
  v1
  $ hg phase -p 9
  $ sort < .hg/store/visibleheads
  96b7359a7ee5350b94be6e5c5dd480751a031498
  f3f5679a1c9cb5a79334a3bbb87b359864c44ce4
  v1
  $ hg phase -p 10
  $ sort < .hg/store/visibleheads
  96b7359a7ee5350b94be6e5c5dd480751a031498
  v1
  $ hg phase -sf 9
  $ sort < .hg/store/visibleheads
  96b7359a7ee5350b94be6e5c5dd480751a031498
  f3f5679a1c9cb5a79334a3bbb87b359864c44ce4
  v1
  $ hg phase -df 8
  $ sort < .hg/store/visibleheads
  96b7359a7ee5350b94be6e5c5dd480751a031498
  f3f5679a1c9cb5a79334a3bbb87b359864c44ce4
  v1
  $ tglogp
  @  10: f3f5679a1c9c secret 'draft4'
  |
  o  9: 5dabc7b08ef9 secret 'draft3'
  |
  o  8: af54c09bb37d draft 'draft2a'
  |
  | o  6: 96b7359a7ee5 draft 'draft1 amend1'
  | |
  o |  2: 4f416a252ac8 public 'public2'
  |/
  o  1: 175dbab47dcc public 'public1'
  |
  o  0: 1e4be0697311 public 'root'
  
  $ hg merge -q 6
  $ hg commit -m "merge1"
  $ hg up -q 6
  $ hg merge -q 10
  $ hg commit -m "merge2"
  $ tglogp
  @    12: 8a541e4b5b52 secret 'merge2'
  |\
  +---o  11: 00c8b0f0741e secret 'merge1'
  | |/
  | o  10: f3f5679a1c9c secret 'draft4'
  | |
  | o  9: 5dabc7b08ef9 secret 'draft3'
  | |
  | o  8: af54c09bb37d draft 'draft2a'
  | |
  o |  6: 96b7359a7ee5 draft 'draft1 amend1'
  | |
  | o  2: 4f416a252ac8 public 'public2'
  |/
  o  1: 175dbab47dcc public 'public1'
  |
  o  0: 1e4be0697311 public 'root'
  
  $ sort < .hg/store/visibleheads
  00c8b0f0741e6ef0696abd63aba22f3d49018b38
  8a541e4b5b528ca9db5d1f8afd4f2534fcd79527
  v1

  $ hg phase -p 11
  $ sort < .hg/store/visibleheads
  8a541e4b5b528ca9db5d1f8afd4f2534fcd79527
  v1
  $ hg phase -p 12
  $ sort < .hg/store/visibleheads
  v1
  $ hg phase -df 11
  $ sort < .hg/store/visibleheads
  00c8b0f0741e6ef0696abd63aba22f3d49018b38
  v1
  $ hg phase -df 10
  $ sort < .hg/store/visibleheads
  00c8b0f0741e6ef0696abd63aba22f3d49018b38
  8a541e4b5b528ca9db5d1f8afd4f2534fcd79527
  v1
  $ hg phase -df 1
  $ sort < .hg/store/visibleheads
  00c8b0f0741e6ef0696abd63aba22f3d49018b38
  8a541e4b5b528ca9db5d1f8afd4f2534fcd79527
  v1
  $ tglogp
  @    12: 8a541e4b5b52 draft 'merge2'
  |\
  +---o  11: 00c8b0f0741e draft 'merge1'
  | |/
  | o  10: f3f5679a1c9c draft 'draft4'
  | |
  | o  9: 5dabc7b08ef9 draft 'draft3'
  | |
  | o  8: af54c09bb37d draft 'draft2a'
  | |
  o |  6: 96b7359a7ee5 draft 'draft1 amend1'
  | |
  | o  2: 4f416a252ac8 draft 'public2'
  |/
  o  1: 175dbab47dcc draft 'public1'
  |
  o  0: 1e4be0697311 public 'root'
  
Hide and unhide

  $ hg up -q 0
  $ hg hide 11
  hiding commit 00c8b0f0741e "merge1"
  1 changesets hidden
  $ sort < .hg/store/visibleheads
  8a541e4b5b528ca9db5d1f8afd4f2534fcd79527
  v1
  $ hg hide 8
  hiding commit af54c09bb37d "draft2a"
  hiding commit 5dabc7b08ef9 "draft3"
  hiding commit f3f5679a1c9c "draft4"
  hiding commit 8a541e4b5b52 "merge2"
  4 changesets hidden
  $ sort < .hg/store/visibleheads
  4f416a252ac81004d9b35542cb1dc8892b6879eb
  96b7359a7ee5350b94be6e5c5dd480751a031498
  v1
  $ hg unhide 9
  $ sort < .hg/store/visibleheads
  5dabc7b08ef934b9e6720285205b2c17695f6491
  96b7359a7ee5350b94be6e5c5dd480751a031498
  v1
  $ hg hide 2 6
  hiding commit 4f416a252ac8 "public2"
  hiding commit 96b7359a7ee5 "draft1 amend1"
  hiding commit af54c09bb37d "draft2a"
  hiding commit 5dabc7b08ef9 "draft3"
  4 changesets hidden
  $ sort < .hg/store/visibleheads
  175dbab47dccefd3ece5916c4f92a6c69f65fcf0
  v1
  $ hg unhide 6
  $ sort < .hg/store/visibleheads
  96b7359a7ee5350b94be6e5c5dd480751a031498
  v1
  $ hg hide 1
  hiding commit 175dbab47dcc "public1"
  hiding commit 96b7359a7ee5 "draft1 amend1"
  2 changesets hidden
  $ sort < .hg/store/visibleheads
  v1
  $ hg unhide 11
  $ sort < .hg/store/visibleheads
  00c8b0f0741e6ef0696abd63aba22f3d49018b38
  v1
  $ hg unhide 12
  $ sort < .hg/store/visibleheads
  00c8b0f0741e6ef0696abd63aba22f3d49018b38
  8a541e4b5b528ca9db5d1f8afd4f2534fcd79527
  v1

Stack navigation and rebases

  $ newrepo
  $ drawdag << EOS
  > E
  > |
  > D
  > |
  > C
  > |
  > B
  > |
  > A
  > EOS
  $ hg up $B
  2 files updated, 0 files merged, 0 files removed, 0 files unresolved
  $ hg amend -m "B amended" --no-rebase
  hint[amend-restack]: descendants of 112478962961 are left behind - use 'hg restack' to rebase them
  hint[hint-ack]: use 'hg hint --ack amend-restack' to silence these hints
  $ tglogm
  @  5: e60094faeb72 'B amended'
  |
  | o  4: 9bc730a19041 'E'
  | |
  | o  3: f585351a92f8 'D'
  | |
  | o  2: 26805aba1e60 'C'
  | |
  | x  1: 112478962961 'B'  (Amended as e60094faeb72)
  |/
  o  0: 426bada5c675 'A'
  
  $ hg next --rebase
  rebasing 2:26805aba1e60 "C"
  1 files updated, 0 files merged, 0 files removed, 0 files unresolved
  [23910a] C
  $ tglogm
  @  6: 23910a6fe564 'C'
  |
  o  5: e60094faeb72 'B amended'
  |
  | o  4: 9bc730a19041 'E'
  | |
  | o  3: f585351a92f8 'D'
  | |
  | x  2: 26805aba1e60 'C'  (Rebased to 23910a6fe564)
  | |
  | x  1: 112478962961 'B'  (Amended as e60094faeb72)
  |/
  o  0: 426bada5c675 'A'
  
  $ hg next --rebase
  rebasing 3:f585351a92f8 "D"
  1 files updated, 0 files merged, 0 files removed, 0 files unresolved
  [1d30cc] D
  $ tglogm
  @  7: 1d30cc995ea7 'D'
  |
  o  6: 23910a6fe564 'C'
  |
  o  5: e60094faeb72 'B amended'
  |
  | o  4: 9bc730a19041 'E'
  | |
  | x  3: f585351a92f8 'D'  (Rebased to 1d30cc995ea7)
  | |
  | x  2: 26805aba1e60 'C'  (Rebased to 23910a6fe564)
  | |
  | x  1: 112478962961 'B'  (Amended as e60094faeb72)
  |/
  o  0: 426bada5c675 'A'
  
  $ hg next --rebase
  rebasing 4:9bc730a19041 "E"
  1 files updated, 0 files merged, 0 files removed, 0 files unresolved
  [ec992f] E
  $ tglogm
  @  8: ec992ff1fd78 'E'
  |
  o  7: 1d30cc995ea7 'D'
  |
  o  6: 23910a6fe564 'C'
  |
  o  5: e60094faeb72 'B amended'
  |
  o  0: 426bada5c675 'A'
  

Undo

  $ hg undo
  undone to *, before next --rebase (glob)
  $ tglogm
  @  7: 1d30cc995ea7 'D'
  |
  o  6: 23910a6fe564 'C'
  |
  o  5: e60094faeb72 'B amended'
  |
  | o  4: 9bc730a19041 'E'
  | |
  | x  3: f585351a92f8 'D'  (Rebased to 1d30cc995ea7)
  | |
  | x  2: 26805aba1e60 'C'  (Rebased to 23910a6fe564)
  | |
  | x  1: 112478962961 'B'  (Amended as e60094faeb72)
  |/
  o  0: 426bada5c675 'A'
  
  $ hg undo
  undone to *, before next --rebase (glob)
  $ tglogm
  @  6: 23910a6fe564 'C'
  |
  o  5: e60094faeb72 'B amended'
  |
  | o  4: 9bc730a19041 'E'
  | |
  | o  3: f585351a92f8 'D'
  | |
  | x  2: 26805aba1e60 'C'  (Rebased to 23910a6fe564)
  | |
  | x  1: 112478962961 'B'  (Amended as e60094faeb72)
  |/
  o  0: 426bada5c675 'A'
  
  $ hg undo
  undone to *, before next --rebase (glob)
  $ tglogm
  @  5: e60094faeb72 'B amended'
  |
  | o  4: 9bc730a19041 'E'
  | |
  | o  3: f585351a92f8 'D'
  | |
  | o  2: 26805aba1e60 'C'
  | |
  | x  1: 112478962961 'B'  (Amended as e60094faeb72)
  |/
  o  0: 426bada5c675 'A'
  
Viewing the log graph with filtering disabled shows the commits that have been undone
from as invisible commits.
  $ tglogm --hidden
  -  8: ec992ff1fd78 'E'
  |
  -  7: 1d30cc995ea7 'D'
  |
  -  6: 23910a6fe564 'C'
  |
  @  5: e60094faeb72 'B amended'
  |
  | o  4: 9bc730a19041 'E'
  | |
  | o  3: f585351a92f8 'D'
  | |
  | o  2: 26805aba1e60 'C'
  | |
  | x  1: 112478962961 'B'  (Amended as e60094faeb72)
  |/
  o  0: 426bada5c675 'A'
  
Unhiding them reveals them as new commits and now the old ones show their relationship
to the new ones.
  $ hg unhide ec992ff1fd78
  $ tglogm
  o  8: ec992ff1fd78 'E'
  |
  o  7: 1d30cc995ea7 'D'
  |
  o  6: 23910a6fe564 'C'
  |
  @  5: e60094faeb72 'B amended'
  |
  | x  4: 9bc730a19041 'E'  (Rebased to ec992ff1fd78)
  | |
  | x  3: f585351a92f8 'D'  (Rebased to 1d30cc995ea7)
  | |
  | x  2: 26805aba1e60 'C'  (Rebased to 23910a6fe564)
  | |
  | x  1: 112478962961 'B'  (Amended as e60094faeb72)
  |/
  o  0: 426bada5c675 'A'
  
