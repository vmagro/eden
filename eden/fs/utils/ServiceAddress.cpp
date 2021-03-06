/*
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * This software may be used and distributed according to the terms of the
 * GNU General Public License version 2.
 */

#include "eden/fs/utils/ServiceAddress.h"

#include <folly/Random.h>
#include <folly/SocketAddress.h>
#include <folly/String.h>
#include <folly/logging/xlog.h>
#include <optional>

#include "eden/fs/eden-config.h"

#ifdef EDEN_HAVE_SERVICEROUTER
#include <servicerouter/client/cpp2/ServiceRouter.h> // @manual
#endif

namespace facebook {
namespace eden {

ServiceAddress::ServiceAddress(std::string name) : name_(std::move(name)) {}

ServiceAddress::ServiceAddress(std::string hostname, uint16_t port)
    : name_(std::make_pair(std::move(hostname), port)) {}

std::optional<SocketAddressWithHostname>
ServiceAddress::getSocketAddressBlocking() {
  if (std::holds_alternative<HostPortPair>(name_)) {
    return addressFromHostname();
  }
  return addressFromSMCTier();
}

std::optional<SocketAddressWithHostname> ServiceAddress::addressFromHostname() {
  auto hostPort = std::get<HostPortPair>(name_);
  auto addr = folly::SocketAddress();
  addr.setFromHostPort(hostPort.first, hostPort.second);
  return std::make_pair(addr, hostPort.first);
}

std::optional<SocketAddressWithHostname> ServiceAddress::addressFromSMCTier(
    std::shared_ptr<facebook::servicerouter::ServiceCacheIf> selector) {
#ifdef EDEN_HAVE_SERVICEROUTER
  auto tier = std::get<std::string>(name_);
  XLOG(DBG7) << "resolving with SMC tier: " << tier;
  auto selection = selector->getSelection_DEPRECATED(
      "SRSelection_CODEMOD_ServiceAddress_50", tier);

  if (selection.hosts->empty()) {
    XLOG(DBG5) << "resolution of SMC tier: " << tier
               << "failed because ServiceRouter returned empty selection";
    return std::nullopt;
  }

  // TODO(zeyi, t42568801): better host selection algorithm
  auto selected = folly::Random::rand32(selection.hosts->size());
  const auto& host = selection.hosts->at(selected);
  auto location = host->location();

  return std::make_pair(
      folly::SocketAddress(location.getIpAddress(), location.getPort()),
      location.getHostname());
#else
  (void)selector;
  return std::nullopt;
#endif
}

std::optional<SocketAddressWithHostname> ServiceAddress::addressFromSMCTier() {
#ifdef EDEN_HAVE_SERVICEROUTER
  auto& factory = servicerouter::cpp2::getClientFactory();
  auto selector = factory.getSelector();

  return addressFromSMCTier(selector);
#else
  XLOG(ERR) << "EdenFS is compiled without ServiceRouter support!";
  return std::nullopt;
#endif
}

} // namespace eden
} // namespace facebook
