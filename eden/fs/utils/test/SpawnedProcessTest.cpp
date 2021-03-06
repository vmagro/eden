/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

#include "eden/fs/utils/SpawnedProcess.h"
#include <folly/String.h>
#include <folly/logging/xlog.h>
#include <folly/portability/GTest.h>
#include <folly/test/TestUtils.h>
#include <gtest/gtest.h>
#include <list>
#include "eden/fs/utils/PathFuncs.h"

using namespace facebook::eden;
using Environment = SpawnedProcess::Environment;
using Options = SpawnedProcess::Options;

#ifndef _WIN32
TEST(SpawnedProcess, cwd_slash) {
  Options opts;
  opts.nullStdin();
  opts.pipeStdout();
  opts.chdir("/"_abspath);
  SpawnedProcess proc({"pwd"}, std::move(opts));

  auto outputs = proc.communicate();
  proc.wait();

  EXPECT_EQ("/\n", outputs.first);
}

TEST(SpawnedProcess, cwd_inherit) {
  Options opts;
  opts.nullStdin();
  opts.pipeStdout();
  SpawnedProcess proc({"pwd"}, std::move(opts));

  auto outputs = proc.communicate();
  proc.wait();

  char cwd[1024];
  getcwd(cwd, sizeof(cwd) - 1);
  strcat(cwd, "\n");

  EXPECT_EQ(cwd, outputs.first);
}
#endif

TEST(SpawnedProcess, pipe) {
  Options opts;
  opts.nullStdin();
  opts.pipeStdout();
  SpawnedProcess echo(
      {
#ifndef _WIN32
          "echo",
#else
          // If we're being built via cmake we know that we
          // have the cmake executable on hand to invoke its
          // echo program
          "cmake",
          "-E",
          "echo",
#endif
          "hello"},
      std::move(opts));

  auto outputs = echo.communicate();
  echo.wait();

  folly::StringPiece line(outputs.first);
  EXPECT_TRUE(line.startsWith("hello"));
}

void test_pipe_input(bool threaded) {
#ifndef _WIN32
  Options opts;
  opts.pipeStdout();
  opts.pipeStdin();
  SpawnedProcess cat({"cat", "-"}, std::move(opts));

  std::vector<std::string> expected{"one", "two", "three"};
  std::list<std::string> lines{"one\n", "two\n", "three\n"};

  auto writable = [&lines](FileDescriptor& fd) {
    if (lines.empty()) {
      return true;
    }
    auto str = lines.front();
    if (write(fd.fd(), str.data(), str.size()) == -1) {
      throw std::runtime_error("write to child failed");
    }
    lines.pop_front();
    return false;
  };

  auto outputs =
      threaded ? cat.threadedCommunicate(writable) : cat.communicate(writable);
  cat.wait();

  std::vector<std::string> resultLines;
  folly::split('\n', outputs.first, resultLines, /*ignoreEmpty=*/true);
  EXPECT_EQ(resultLines.size(), 3);
  EXPECT_EQ(resultLines, expected);
#else
  (void)threaded;
#endif
}

TEST(SpawnedProcess, stresstest_pipe_output) {
  bool okay = true;
#ifndef _WIN32
  for (int i = 0; i < 3000; ++i) {
    Options opts;
    opts.pipeStdout();
    opts.nullStdin();
    SpawnedProcess proc({"head", "-n20", "/dev/urandom"}, std::move(opts));
    auto outputs = proc.communicate();
    folly::StringPiece out(outputs.first);
    proc.wait();
    if (out.empty() || out[out.size() - 1] != '\n') {
      okay = false;
      break;
    }
  }
#endif
  EXPECT_TRUE(okay);
}

TEST(SpawnedProcess, inputThreaded) {
  test_pipe_input(true);
}

TEST(SpawnedProcess, inputNotThreaded) {
  test_pipe_input(false);
}
