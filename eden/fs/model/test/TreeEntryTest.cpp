/*
 *  Copyright (c) 2016-present, Facebook, Inc.
 *  All rights reserved.
 *
 *  This source code is licensed under the BSD-style license found in the
 *  LICENSE file in the root directory of this source tree. An additional grant
 *  of patent rights can be found in the PATENTS file in the same directory.
 *
 */
#include "eden/fs/model/TreeEntry.h"
#include "eden/fs/testharness/TestUtil.h"

#include <gtest/gtest.h>

using namespace facebook::eden;

TEST(TreeEntry, modeAndLogString) {
  TreeEntry rwFile(
      makeTestHash("faceb00c"), "file.txt", FileType::REGULAR_FILE);
  EXPECT_EQ(S_IFREG | S_IRUSR | S_IWUSR | S_IRGRP | S_IROTH, rwFile.getMode());
  EXPECT_EQ(
      "(file.txt, 00000000000000000000000000000000faceb00c, f)",
      rwFile.toLogString());

  TreeEntry rwxFile(makeTestHash("789"), "file.exe", FileType::EXECUTABLE_FILE);
  EXPECT_EQ(
      S_IFREG | S_IRWXU | S_IRGRP | S_IXGRP | S_IROTH | S_IXOTH,
      rwxFile.getMode());
  EXPECT_EQ(
      "(file.exe, 0000000000000000000000000000000000000789, x)",
      rwxFile.toLogString());

  TreeEntry rwxLink(makeTestHash("b"), "to-file.exe", FileType::SYMLINK);
  EXPECT_EQ(
      S_IFLNK | S_IRWXU | S_IRGRP | S_IXGRP | S_IROTH | S_IXOTH,
      rwxLink.getMode());
  EXPECT_EQ(
      "(to-file.exe, 000000000000000000000000000000000000000b, l)",
      rwxLink.toLogString());

  TreeEntry directory(makeTestHash("abc"), "src", FileType::DIRECTORY);
  EXPECT_EQ(
      S_IFDIR | S_IRWXU | S_IRGRP | S_IXGRP | S_IROTH | S_IXOTH,
      directory.getMode());
  EXPECT_EQ(
      "(src, 0000000000000000000000000000000000000abc, d)",
      directory.toLogString());
}
