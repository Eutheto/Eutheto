// SPDX-License-Identifier: Apache-2.0
#include "worker.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstdint>
#include <istream>
#include <limits>
#include <mutex>
#include <new>
#include <optional>
#include <ostream>
#include <set>
#include <string>
#include <string_view>
#include <thread>
#include <utility>
#include <vector>

#include "ortools/base/version.h"
#include "ortools/sat/cp_model.pb.h"
#include "ortools/sat/cp_model_checker.h"
#include "ortools/sat/cp_model_solver.h"
#include "ortools/sat/model.h"
#include "ortools/sat/parameters_validation.h"
#include "ortools/sat/sat_parameters.pb.h"
#include "protocol-policy.h"
#include "wire.h"

namespace eutheto::ortools_worker {
namespace {

namespace protocol = eutheto::worker::v1;
namespace policy = eutheto::worker::v1::policy;
namespace sat = operations_research::sat;

constexpr std::string_view kWorkerIdentity = "eutheto-ortools-worker";
constexpr std::string_view kWorkerVersion = "0.1.0";
constexpr std::string_view kAdapterVersion = "0.1.0";
constexpr std::string_view kBackendId = "ortools-cp-sat";
constexpr std::string_view kOrtoolsVersion = "9.15.6755";

constexpr std::array<protocol::Capability, 8> kCapabilities = {
    protocol::CAPABILITY_CP_SAT,
    protocol::CAPABILITY_INTERMEDIATE_SOLUTIONS,
    protocol::CAPABILITY_PROGRESS,
    protocol::CAPABILITY_SOLUTION_PROJECTION,
    protocol::CAPABILITY_OBJECTIVE_BOUNDS,
    protocol::CAPABILITY_SOLUTION_STATS,
    protocol::CAPABILITY_SUFFICIENT_ASSUMPTIONS,
    protocol::CAPABILITY_DETERMINISTIC_TIME};

void StoreBe64(std::uint64_t value, std::uint8_t* output) {
  for (unsigned index = 0; index < 8; ++index)
    output[7 - index] = static_cast<std::uint8_t>(value >> (index * 8U));
}

void StoreBe32(std::uint32_t value, std::uint8_t* output) {
  for (unsigned index = 0; index < 4; ++index)
    output[3 - index] = static_cast<std::uint8_t>(value >> (index * 8U));
}

bool IsIdentifierCharacter(char value) {
  return (value >= 'A' && value <= 'Z') ||
         (value >= 'a' && value <= 'z') ||
         (value >= '0' && value <= '9') || value == '-';
}

bool ParseCoreNumber(std::string_view value, std::size_t* position) {
  const std::size_t start = *position;
  std::uint64_t number = 0;
  while (*position < value.size() && value[*position] >= '0' &&
         value[*position] <= '9') {
    const std::uint64_t digit =
        static_cast<std::uint64_t>(value[*position] - '0');
    if (number > (std::numeric_limits<std::uint64_t>::max() - digit) / 10U)
      return false;
    number = number * 10U + digit;
    ++*position;
  }
  return *position != start &&
         !(*position - start > 1 && value[start] == '0');
}

bool ParseIdentifiers(std::string_view value, std::size_t* position,
                      bool reject_numeric_leading_zero) {
  for (;;) {
    const std::size_t start = *position;
    bool numeric = true;
    while (*position < value.size() && value[*position] != '.' &&
           value[*position] != '+') {
      const char character = value[*position];
      if (!IsIdentifierCharacter(character)) return false;
      numeric = numeric && character >= '0' && character <= '9';
      ++*position;
    }
    if (*position == start ||
        (reject_numeric_leading_zero && numeric &&
         *position - start > 1 && value[start] == '0')) {
      return false;
    }
    if (*position == value.size() || value[*position] == '+') return true;
    ++*position;
  }
}

bool IsSemverCore(std::string_view value) {
  if (value.empty() ||
      value.size() > policy::kHandshakeRequestCoreVersionMaxBytes)
    return false;
  std::size_t position = 0;
  for (unsigned component = 0; component < 3; ++component) {
    if (!ParseCoreNumber(value, &position)) return false;
    if (component != 2) {
      if (position == value.size() || value[position] != '.') return false;
      ++position;
    }
  }
  if (position < value.size() && value[position] == '-') {
    ++position;
    if (!ParseIdentifiers(value, &position, true)) return false;
  }
  if (position < value.size() && value[position] == '+') {
    ++position;
    if (!ParseIdentifiers(value, &position, false)) return false;
  }
  return position == value.size();
}

bool ConstantTimeEqual(std::span<const std::uint8_t> left,
                       std::string_view right) {
  if (left.size() != right.size()) return false;
  std::uint8_t difference = 0;
  for (std::size_t index = 0; index < left.size(); ++index)
    difference |= left[index] ^ static_cast<std::uint8_t>(right[index]);
  return difference == 0;
}

protocol::WorkerFrame HandshakeFailure(protocol::HandshakeErrorCode code,
                                       std::string_view message,
                                       bool include_supported = false) {
  protocol::WorkerFrame frame;
  auto* error = frame.mutable_handshake_response()->mutable_error();
  error->set_code(code);
  error->set_message(message);
  if (include_supported) {
    error->set_supported_protocol_major(policy::kProtocolMajor);
    error->set_supported_protocol_minor(policy::kProtocolMinor);
  }
  return frame;
}

std::optional<protocol::WorkerFrame> CheckHandshake(
    const protocol::ParentFrame& parent) {
  if (!parent.has_handshake_request()) {
    return HandshakeFailure(protocol::HANDSHAKE_ERROR_CODE_INVALID_VERSION,
                            "expected one handshake request");
  }
  const auto& request = parent.handshake_request();
  if (request.protocol_major() != policy::kProtocolMajor) {
    return HandshakeFailure(
        protocol::HANDSHAKE_ERROR_CODE_UNSUPPORTED_PROTOCOL_MAJOR,
        "unsupported protocol major", true);
  }
  if (request.protocol_minor() != policy::kProtocolMinor) {
    return HandshakeFailure(
        protocol::HANDSHAKE_ERROR_CODE_UNSUPPORTED_PROTOCOL_MINOR,
        "unsupported protocol minor", true);
  }
  if (!IsSemverCore(request.core_version())) {
    return HandshakeFailure(protocol::HANDSHAKE_ERROR_CODE_INVALID_VERSION,
                            "core version must be a bounded semantic version");
  }
  if (request.expected_backend_id() != kBackendId) {
    return HandshakeFailure(protocol::HANDSHAKE_ERROR_CODE_UNEXPECTED_BACKEND,
                            "requested backend does not match this worker");
  }
  if (request.expected_manifest_sha256().size() != 32) {
    return HandshakeFailure(protocol::HANDSHAKE_ERROR_CODE_MANIFEST_MISMATCH,
                            "manifest correlation digest must be 32 bytes");
  }
  std::set<int> required;
  for (const int capability : request.required_capabilities()) {
    if (capability == protocol::CAPABILITY_UNSPECIFIED ||
        !required.insert(capability).second ||
        std::find(kCapabilities.begin(), kCapabilities.end(), capability) ==
            kCapabilities.end()) {
      return HandshakeFailure(
          protocol::HANDSHAKE_ERROR_CODE_MISSING_CAPABILITY,
          "required capabilities must be known, unique, and supported");
    }
  }
  return std::nullopt;
}

protocol::WorkerFrame HandshakeSuccess(
    const protocol::HandshakeRequest& request) {
  protocol::WorkerFrame frame;
  auto* success = frame.mutable_handshake_response()->mutable_success();
  success->set_protocol_major(policy::kProtocolMajor);
  success->set_protocol_minor(policy::kProtocolMinor);
  success->set_worker_identity(kWorkerIdentity);
  success->set_worker_version(kWorkerVersion);
  success->set_backend_id(kBackendId);
  success->set_ortools_version(kOrtoolsVersion);
  success->set_adapter_version(kAdapterVersion);
  success->set_manifest_sha256(request.expected_manifest_sha256());
  for (const auto capability : kCapabilities) success->add_capabilities(capability);
  return frame;
}

protocol::WorkerFrame WorkerFailure(std::string_view request_id,
                                    protocol::WorkerErrorCode code,
                                    std::string_view message,
                                    bool retryable = false) {
  protocol::WorkerFrame frame;
  auto* error = frame.mutable_error();
  error->set_request_id(request_id);
  error->set_code(code);
  error->set_message(message);
  error->set_retryable(retryable);
  return frame;
}

bool ParseParent(const std::vector<std::uint8_t>& payload,
                 protocol::ParentFrame* frame) {
  std::string reason;
  if (!PreflightParentFrame(payload, &reason)) return false;
  return frame->ParseFromArray(payload.data(), static_cast<int>(payload.size()));
}

bool IsFiniteNonnegative(double value) {
  return std::isfinite(value) && value >= 0.0;
}

bool HasObjective(const sat::CpModelProto& model) {
  return model.has_objective() || model.has_floating_point_objective();
}

struct AssumptionProfile {
  bool enabled = false;
  // Sorted submitted literals make duplicate and exact signed-membership
  // checks non-quadratic. This buffer is retained across model validation and
  // terminal admission.
  std::vector<std::int32_t> sorted_submitted;
};

bool IsValidLiteral(std::int32_t literal, int variable_count) {
  const std::int64_t value = literal;
  const std::int64_t variable = value >= 0 ? value : -value - 1;
  return variable >= 0 && variable < variable_count;
}

std::optional<protocol::WorkerFrame> ValidateAssumptionProfile(
    const protocol::SolveRequest& request, const sat::CpModelProto& model,
    const AppliedParameters& applied, AssumptionProfile* profile) {
  profile->enabled = false;
  profile->sorted_submitted.clear();
  if (model.assumptions_size() == 0) return std::nullopt;

  if (static_cast<std::size_t>(model.assumptions_size()) >
      policy::kMaxRepeatedFieldItems) {
    return WorkerFailure(
        request.request_id(), protocol::WORKER_ERROR_CODE_INVALID_MODEL,
        "model assumption count exceeds the protocol bound");
  }
  if (HasObjective(model)) {
    return WorkerFailure(
        request.request_id(), protocol::WORKER_ERROR_CODE_UNSUPPORTED_MODEL,
        "nonempty assumptions require a satisfaction model");
  }
  if (applied.worker_threads != 1) {
    return WorkerFailure(
        request.request_id(), protocol::WORKER_ERROR_CODE_INVALID_PARAMETERS,
        "nonempty assumptions require exactly one worker");
  }

  profile->enabled = true;
  profile->sorted_submitted.reserve(
      static_cast<std::size_t>(model.assumptions_size()));
  for (const std::int32_t literal : model.assumptions()) {
    if (!IsValidLiteral(literal, model.variables_size())) {
      profile->enabled = false;
      profile->sorted_submitted.clear();
      return WorkerFailure(
          request.request_id(), protocol::WORKER_ERROR_CODE_INVALID_MODEL,
          "model assumptions contain an out-of-range literal");
    }
    profile->sorted_submitted.push_back(literal);
  }
  std::sort(profile->sorted_submitted.begin(), profile->sorted_submitted.end());
  if (std::adjacent_find(profile->sorted_submitted.begin(),
                         profile->sorted_submitted.end()) !=
      profile->sorted_submitted.end()) {
    profile->enabled = false;
    profile->sorted_submitted.clear();
    return WorkerFailure(
        request.request_id(), protocol::WORKER_ERROR_CODE_INVALID_MODEL,
        "model assumptions contain a duplicate literal");
  }
  return std::nullopt;
}

bool ContainsModelText(const sat::CpModelProto& model) {
  if (!model.name().empty()) return true;
  for (const auto& variable : model.variables()) {
    if (!variable.name().empty()) return true;
  }
  for (const auto& constraint : model.constraints()) {
    if (!constraint.name().empty()) return true;
  }
  return false;
}

void AddObjectiveEvidence(const sat::CpModelProto& model,
                          const sat::CpSolverResponse& response,
                          google::protobuf::RepeatedField<double>* objective,
                          google::protobuf::RepeatedField<double>* bound) {
  if (!HasObjective(model)) return;
  if (std::isfinite(response.objective_value()))
    objective->Add(response.objective_value());
  if (std::isfinite(response.best_objective_bound()))
    bound->Add(response.best_objective_bound());
}

bool AddProjection(const protocol::SolveRequest& request,
                   const sat::CpSolverResponse& response,
                   protocol::ProjectedCandidate* candidate) {
  if (response.solution_size() == 0 && request.projections_size() != 0)
    return false;
  for (const auto& projection : request.projections()) {
    const int index = projection.cp_sat_variable_index();
    if (index < 0 || index >= response.solution_size()) return false;
    auto* value = candidate->add_values();
    value->set_projection_id(projection.projection_id());
    value->set_value(response.solution(index));
  }
  return true;
}

bool WriteSessionFrame(std::ostream& output,
                       const protocol::WorkerFrame& frame,
                       std::uint64_t* output_bytes) {
  const std::uint64_t framed_size =
      policy::kLengthPrefixBytes + frame.ByteSizeLong();
  if (framed_size > policy::kTotalSessionBytes - *output_bytes) return false;
  if (!WriteFrame(output, frame)) return false;
  *output_bytes += framed_size;
  return true;
}

class EventWriter final {
 public:
  EventWriter(std::ostream& output, std::uint64_t* output_bytes)
      : output_(output),
        output_bytes_(output_bytes),
        thread_([this] { Run(); }) {}

  ~EventWriter() {
    if (thread_.joinable()) {
      {
        std::lock_guard lock(mutex_);
        closing_ = true;
      }
      condition_.notify_one();
      thread_.join();
    }
  }

  EventWriter(const EventWriter&) = delete;
  EventWriter& operator=(const EventWriter&) = delete;

  bool WantsFirstIncumbent() const noexcept {
    return !failed_.load(std::memory_order_relaxed) &&
           !first_seen_.load(std::memory_order_acquire);
  }

  void Offer(protocol::WorkerFrame frame, bool incumbent) {
    if (failed_.load()) return;

    if (incumbent) {
      // Only the first projected incumbent is consumed by the adapter. Later
      // solutions remain the worker's private optimization state until Finished.
      if (first_seen_.load(std::memory_order_acquire) ||
          !frame.has_incumbent() || !frame.incumbent().has_candidate())
        return;
      // The writer never performs I/O under this mutex. Briefly acquiring it
      // makes the first projected solution reliable without a wakeup race.
      std::lock_guard lock(mutex_);
      if (closing_ || first_seen_.load(std::memory_order_relaxed) ||
          admitted_ >= policy::kEventsPerSession)
        return;
      first_incumbent_ = std::move(frame);
      first_seen_.store(true, std::memory_order_release);
      ++admitted_;
      condition_.notify_one();
      return;
    }

    // Reserve one event for the first solution until it arrives. Progress is
    // still lossy and coalesced, so a busy solver never blocks in this callback.
    std::unique_lock lock(mutex_, std::try_to_lock);
    if (!lock.owns_lock() || closing_) return;
    const std::size_t limit =
        first_seen_.load(std::memory_order_relaxed)
            ? policy::kEventsPerSession
            : policy::kEventsPerSession - 1;
    if (!progress_) {
      if (admitted_ >= limit) return;
      ++admitted_;
    }
    progress_ = std::move(frame);
    condition_.notify_one();
  }

  bool Close() {
    {
      std::lock_guard lock(mutex_);
      closing_ = true;
    }
    condition_.notify_one();
    if (thread_.joinable()) thread_.join();
    return !failed_.load();
  }

 private:
  void Run() noexcept {
    try {
      RunImpl();
    } catch (...) {
      failed_.store(true);
    }
  }

  void RunImpl() {
    const auto interval = std::chrono::microseconds(
        1000000 / static_cast<long long>(policy::kEventsPerSecond));
    auto next_write = std::chrono::steady_clock::now();
    for (;;) {
      std::optional<protocol::WorkerFrame> frame;
      {
        std::unique_lock lock(mutex_);
        condition_.wait(lock, [this] {
          return first_incumbent_.has_value() || progress_.has_value() ||
                 closing_;
        });
        if (first_incumbent_) {
          frame = std::move(first_incumbent_);
          first_incumbent_.reset();
        } else if (progress_) {
          frame = std::move(progress_);
          progress_.reset();
        } else if (closing_) {
          break;
        }
      }
      if (!frame) continue;
      const auto now = std::chrono::steady_clock::now();
      if (now < next_write) std::this_thread::sleep_until(next_write);
      if (!WriteSessionFrame(output_, *frame, output_bytes_)) {
        failed_.store(true);
        std::lock_guard lock(mutex_);
        progress_.reset();
        closing_ = true;
        break;
      }
      next_write = std::chrono::steady_clock::now() + interval;
    }
  }

  std::ostream& output_;
  std::uint64_t* output_bytes_;
  std::mutex mutex_;
  std::condition_variable condition_;
  std::optional<protocol::WorkerFrame> first_incumbent_;
  std::optional<protocol::WorkerFrame> progress_;
  std::atomic<bool> first_seen_{false};
  std::size_t admitted_ = 0;
  bool closing_ = false;
  std::atomic<bool> failed_{false};
  std::thread thread_;
};

protocol::WorkerFrame MakeIncumbent(const protocol::SolveRequest& request,
                                    const sat::CpModelProto& model,
                                    const sat::CpSolverResponse& response) {
  protocol::WorkerFrame frame;
  auto* incumbent = frame.mutable_incumbent();
  incumbent->set_request_id(request.request_id());
  if (!AddProjection(request, response, incumbent->mutable_candidate())) {
    incumbent->clear_candidate();
  }
  AddObjectiveEvidence(model, response,
                       incumbent->mutable_objective_values(),
                       incumbent->mutable_best_bound_values());
  if (IsFiniteNonnegative(response.wall_time()))
    incumbent->set_wall_time_seconds(response.wall_time());
  if (IsFiniteNonnegative(response.deterministic_time()))
    incumbent->set_deterministic_time(response.deterministic_time());
  return frame;
}

protocol::WorkerFrame MakeBoundProgress(std::string_view request_id,
                                        double bound) {
  protocol::WorkerFrame frame;
  auto* progress = frame.mutable_progress();
  progress->set_request_id(request_id);
  progress->set_kind(protocol::PROGRESS_KIND_BOUND_IMPROVED);
  progress->add_best_bound_values(bound);
  return frame;
}

void SetStatus(const AppliedParameters& parameters,
               const sat::CpSolverResponse& response,
               protocol::Finished* finished) {
  finished->set_raw_cp_sat_status(static_cast<int>(response.status()));
  switch (response.status()) {
    case sat::OPTIMAL:
      finished->set_status(protocol::WORKER_SOLVE_STATUS_OPTIMAL);
      finished->set_termination_reason(protocol::TERMINATION_REASON_OPTIMAL);
      break;
    case sat::INFEASIBLE:
      finished->set_status(protocol::WORKER_SOLVE_STATUS_INFEASIBLE);
      finished->set_termination_reason(protocol::TERMINATION_REASON_INFEASIBLE);
      break;
    case sat::MODEL_INVALID:
      finished->set_status(protocol::WORKER_SOLVE_STATUS_INVALID_MODEL);
      finished->set_termination_reason(protocol::TERMINATION_REASON_INVALID_MODEL);
      break;
    case sat::FEASIBLE:
      finished->set_status(protocol::WORKER_SOLVE_STATUS_FEASIBLE);
      finished->set_termination_reason(
          parameters.stop_after_first_feasible
              ? protocol::TERMINATION_REASON_SOLUTION_LIMIT
              : protocol::TERMINATION_REASON_TIME_LIMIT);
      break;
    case sat::UNKNOWN:
    default:
      finished->set_status(protocol::WORKER_SOLVE_STATUS_NO_SOLUTION);
      finished->set_termination_reason(
          response.wall_time() + 1e-9 >=
                  static_cast<double>(parameters.wall_time_millis) / 1000.0
              ? protocol::TERMINATION_REASON_TIME_LIMIT
              : protocol::TERMINATION_REASON_UNKNOWN);
      break;
  }
}

enum class AssumptionCoreResult { kNotPresent, kValid, kInvalid };

AssumptionCoreResult AdmitAssumptionCore(
    sat::CpSolverResponse* response, const AssumptionProfile& profile,
    protocol::Finished* finished) {
  if (!profile.enabled || response->status() != sat::INFEASIBLE)
    return AssumptionCoreResult::kNotPresent;
  auto* core = response->mutable_sufficient_assumptions_for_infeasibility();
  if (core->empty()) return AssumptionCoreResult::kNotPresent;
  if (static_cast<std::size_t>(core->size()) >
          policy::kFinishedSufficientAssumptionsMaxCount ||
      static_cast<std::size_t>(core->size()) >
          profile.sorted_submitted.size()) {
    return AssumptionCoreResult::kInvalid;
  }

  // Canonicalize the solver-owned response field in place. Whole-core
  // duplicate and exact signed-membership checks remain bounded and
  // non-quadratic without mapping literals to positions or allocating a
  // second core buffer.
  std::sort(core->begin(), core->end());
  if (std::adjacent_find(core->begin(), core->end()) != core->end())
    return AssumptionCoreResult::kInvalid;
  for (const std::int32_t literal : *core) {
    if (!std::binary_search(profile.sorted_submitted.begin(),
                            profile.sorted_submitted.end(), literal)) {
      return AssumptionCoreResult::kInvalid;
    }
  }
  for (const std::int32_t literal : *core)
    finished->add_sufficient_assumptions(literal);
  return AssumptionCoreResult::kValid;
}

protocol::WorkerFrame MakeFinished(const protocol::SolveRequest& request,
                                   const sat::CpModelProto& model,
                                   const AppliedParameters& parameters,
                                   const Sha256Digest& applied_hash,
                                   sat::CpSolverResponse& response,
                                   AssumptionProfile* assumption_profile,
                                   bool* invalid_assumption_core) {

  *invalid_assumption_core = false;
  protocol::WorkerFrame frame;
  auto* finished = frame.mutable_finished();
  finished->set_request_id(request.request_id());
  SetStatus(parameters, response, finished);
  if (response.status() == sat::FEASIBLE || response.status() == sat::OPTIMAL) {
    if (!AddProjection(request, response, finished->mutable_final_candidate()))
      finished->clear_final_candidate();
    AddObjectiveEvidence(model, response, finished->mutable_objective_values(),
                         finished->mutable_best_bound_values());
  }
  if (response.status() == sat::INFEASIBLE) {
    *invalid_assumption_core =
        AdmitAssumptionCore(&response, *assumption_profile, finished) ==
        AssumptionCoreResult::kInvalid;
  }
  if (IsFiniteNonnegative(response.wall_time()))
    finished->set_wall_time_seconds(response.wall_time());
  if (IsFiniteNonnegative(response.user_time()))
    finished->set_user_time_seconds(response.user_time());
  if (IsFiniteNonnegative(response.deterministic_time()))
    finished->set_deterministic_time(response.deterministic_time());
  if (response.num_conflicts() >= 0)
    finished->set_conflicts(static_cast<std::uint64_t>(response.num_conflicts()));
  if (response.num_branches() >= 0)
    finished->set_branches(static_cast<std::uint64_t>(response.num_branches()));
  if (response.num_binary_propagations() >= 0)
    finished->set_binary_propagations(
        static_cast<std::uint64_t>(response.num_binary_propagations()));
  if (response.num_integer_propagations() >= 0)
    finished->set_integer_propagations(
        static_cast<std::uint64_t>(response.num_integer_propagations()));
  finished->set_applied_parameters_sha256(applied_hash.data(),
                                           applied_hash.size());
  finished->set_model_fingerprint(request.model_fingerprint());
  return frame;
}

sat::SatParameters BuildSatParameters(const AppliedParameters& applied,
                                      bool assumption_profile) {
  sat::SatParameters parameters;
  parameters.set_max_time_in_seconds(
      static_cast<double>(applied.wall_time_millis) / 1000.0);
  parameters.set_num_workers(static_cast<int>(applied.worker_threads));
  parameters.set_random_seed(applied.random_seed);
  parameters.set_stop_after_first_solution(applied.stop_after_first_feasible);
  parameters.set_enumerate_all_solutions(false);
  parameters.set_log_search_progress(false);
  parameters.set_log_to_stdout(false);
  if (assumption_profile) parameters.set_cp_model_presolve(false);
  return parameters;
}

std::optional<protocol::WorkerFrame> ValidateSolve(
    const protocol::ParentFrame& parent, protocol::SolveRequest const** request,
    sat::CpModelProto* model, AppliedParameters* applied,
    Sha256Digest* applied_hash, AssumptionProfile* assumption_profile,
    sat::SatParameters* final_parameters) {
  if (!parent.has_solve_request()) {
    return WorkerFailure("", protocol::WORKER_ERROR_CODE_PROTOCOL_VIOLATION,
                         "expected exactly one solve request");
  }
  *request = &parent.solve_request();
  const auto& solve = **request;
  if (solve.request_id().empty()) {
    return WorkerFailure("", protocol::WORKER_ERROR_CODE_PROTOCOL_VIOLATION,
                         "request identifier must be nonempty");
  }
  if (solve.model_fingerprint().size() != 32) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_PROTOCOL_VIOLATION,
                         "model fingerprint must be 32 bytes");
  }
  const auto exact_model_bytes = std::span<const std::uint8_t>(
      reinterpret_cast<const std::uint8_t*>(solve.cp_model_proto().data()),
      solve.cp_model_proto().size());
  const Sha256Digest fingerprint = Sha256Bytes(exact_model_bytes);
  if (!ConstantTimeEqual(fingerprint, solve.model_fingerprint())) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_PROTOCOL_VIOLATION,
                         "model fingerprint does not match exact model bytes");
  }
  if (!solve.has_parameters() || !solve.has_resource_limits()) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_INVALID_PARAMETERS,
                         "parameters and resource limits are required");
  }
  const auto& limits = solve.resource_limits();
  if (limits.wall_time_millis() == 0 || limits.worker_threads() == 0 ||
      limits.worker_threads() > policy::kMaxWorkerThreads ||
      (limits.has_memory_bytes() && limits.memory_bytes() == 0)) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_RESOURCE_LIMIT,
                         "resource limits are out of range");
  }
  const auto& parameters = solve.parameters();
  applied->wall_time_millis = limits.wall_time_millis();
  applied->worker_threads = limits.worker_threads();
  applied->random_seed = parameters.has_random_seed() ? parameters.random_seed() : 1;
  applied->stop_after_first_feasible =
      parameters.has_stop_after_first_feasible() &&
      parameters.stop_after_first_feasible();
  applied->emit_intermediate_solutions =
      parameters.has_emit_intermediate_solutions() &&
      parameters.emit_intermediate_solutions();
  applied->log_search_progress = parameters.has_log_search_progress() &&
                                 parameters.log_search_progress();
  applied->deterministic_test_profile =
      parameters.has_deterministic_test_profile() &&
      parameters.deterministic_test_profile();
  if (applied->deterministic_test_profile &&
      (applied->worker_threads != 1 || applied->random_seed != 1)) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_INVALID_PARAMETERS,
                         "deterministic profile requires one worker and seed one");
  }

  std::set<std::uint64_t> projection_ids;
  if (!model->ParseFromArray(solve.cp_model_proto().data(),
                             static_cast<int>(solve.cp_model_proto().size()))) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_INVALID_MODEL,
                         "CP-SAT model bytes are malformed");
  }
  if (ContainsModelText(*model)) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_UNSUPPORTED_MODEL,
                         "CP-SAT model text fields are not supported");
  }
  for (const auto& projection : solve.projections()) {
    if (projection.cp_sat_variable_index() < 0 ||
        projection.cp_sat_variable_index() >= model->variables_size() ||
        !projection_ids.insert(projection.projection_id()).second) {
      return WorkerFailure(
          solve.request_id(), protocol::WORKER_ERROR_CODE_PROTOCOL_VIOLATION,
          "projection identifiers must be unique and indices must be in range");
    }
  }

  if (auto failure =
          ValidateAssumptionProfile(solve, *model, *applied, assumption_profile))
    return failure;

  *final_parameters =
      BuildSatParameters(*applied, assumption_profile->enabled);
  if (!sat::ValidateParameters(*final_parameters).empty()) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_INVALID_PARAMETERS,
                         "normalized CP-SAT parameters are invalid");
  }
  if (!sat::ValidateInputCpModel(*final_parameters, *model).empty()) {
    return WorkerFailure(solve.request_id(),
                         protocol::WORKER_ERROR_CODE_INVALID_MODEL,
                         "CP-SAT model validation failed");
  }
  *applied_hash = AppliedParametersHash(*applied);
  return std::nullopt;
}

}  // namespace

std::array<std::uint8_t, 56> AppliedParametersPreimage(
    const AppliedParameters& parameters) {
  std::array<std::uint8_t, 56> preimage{};
  static_assert(policy::kAppliedParametersHashDomainSeparator.size() == 36);
  std::copy(policy::kAppliedParametersHashDomainSeparator.begin(),
            policy::kAppliedParametersHashDomainSeparator.end(),
            preimage.begin());
  StoreBe64(parameters.wall_time_millis, preimage.data() + 36);
  StoreBe32(parameters.worker_threads, preimage.data() + 44);
  StoreBe32(static_cast<std::uint32_t>(parameters.random_seed),
            preimage.data() + 48);
  preimage[52] = parameters.stop_after_first_feasible ? 1U : 0U;
  preimage[53] = parameters.emit_intermediate_solutions ? 1U : 0U;
  preimage[54] = parameters.log_search_progress ? 1U : 0U;
  preimage[55] = parameters.deterministic_test_profile ? 1U : 0U;
  return preimage;
}

Sha256Digest AppliedParametersHash(const AppliedParameters& parameters) {
  const auto preimage = AppliedParametersPreimage(parameters);
  return Sha256Bytes(preimage);
}

int RunSession(std::istream& input, std::ostream& output,
               std::ostream& diagnostics) {
  (void)diagnostics;
  std::uint64_t output_bytes = 0;
  bool started_written = false;
  std::string terminal_request_id;
  try {
    if (operations_research::OrToolsVersionString() != kOrtoolsVersion)
      return kExitOrtoolsInitialization;


    FrameReader reader(input);
    FrameReadResult handshake_payload =
        reader.Read(policy::kHandshakeMaxPayloadBytes);
    if (handshake_payload.status != FrameReadStatus::kOk) return kExitProtocol;
    protocol::ParentFrame handshake_parent;
    if (!ParseParent(handshake_payload.payload, &handshake_parent))
      return kExitProtocol;
    if (auto failure = CheckHandshake(handshake_parent)) {
      return WriteSessionFrame(output, *failure, &output_bytes) ? 0
                                                                : kExitOutput;
    }
    const protocol::WorkerFrame handshake_success =
        HandshakeSuccess(handshake_parent.handshake_request());
    if (!WriteSessionFrame(output, handshake_success, &output_bytes))
      return kExitOutput;

    FrameReadResult solve_payload =
        reader.Read(policy::kSolveRequestMaxPayloadBytes);
    if (solve_payload.status != FrameReadStatus::kOk) return kExitProtocol;
    protocol::ParentFrame solve_parent;
    if (!ParseParent(solve_payload.payload, &solve_parent))
      return kExitProtocol;
    if (!solve_parent.has_solve_request() ||
        solve_parent.solve_request().request_id().empty() ||
        solve_parent.solve_request().model_fingerprint().size() != 32) {
      return kExitProtocol;
    }
    const auto& decoded_solve = solve_parent.solve_request();
    terminal_request_id = decoded_solve.request_id();
    protocol::WorkerFrame started;
    started.mutable_started()->set_request_id(decoded_solve.request_id());
    started.mutable_started()->set_model_fingerprint(
        decoded_solve.model_fingerprint());
    if (!WriteSessionFrame(output, started, &output_bytes)) return kExitOutput;
    started_written = true;
    const FrameReadStatus trailing = reader.CheckEof();
    if (trailing != FrameReadStatus::kEof) {
      auto error = WorkerFailure(
          terminal_request_id,
          trailing == FrameReadStatus::kOk
              ? protocol::WORKER_ERROR_CODE_PROTOCOL_VIOLATION
              : protocol::WORKER_ERROR_CODE_MALFORMED_FRAME,
          "exactly one complete solve frame followed by EOF is required");
      return WriteSessionFrame(output, error, &output_bytes) ? 0 : kExitOutput;
    }

    const protocol::SolveRequest* request = nullptr;
    sat::CpModelProto model;
    AppliedParameters applied;
    Sha256Digest applied_hash{};
    AssumptionProfile assumption_profile;
    sat::SatParameters final_parameters;
    if (auto failure =
            ValidateSolve(solve_parent, &request, &model, &applied,
                          &applied_hash, &assumption_profile, &final_parameters)) {
      return WriteSessionFrame(output, *failure, &output_bytes) ? 0
                                                                : kExitOutput;
    }

    std::atomic<bool> callback_failed{false};
    EventWriter events(output, &output_bytes);
    sat::Model solver_model;
    solver_model.Add(sat::NewSatParameters(final_parameters));


    if (applied.emit_intermediate_solutions) {
      solver_model.Add(sat::NewFeasibleSolutionObserver(
          [&events, &callback_failed, request,
           &model](const sat::CpSolverResponse& response) {
            try {
              if (events.WantsFirstIncumbent())
                events.Offer(MakeIncumbent(*request, model, response), true);
            } catch (...) {
              callback_failed.store(true);
            }
          }));
    }
    if (applied.log_search_progress && HasObjective(model)) {
      const std::string callback_request_id = request->request_id();
      solver_model.Add(sat::NewBestBoundCallback(
          [&events, &callback_failed, callback_request_id](double bound) {
            try {
              if (std::isfinite(bound))
                events.Offer(MakeBoundProgress(callback_request_id, bound),
                             false);
            } catch (...) {
              callback_failed.store(true);
            }
          }));
    }

    sat::CpSolverResponse response = sat::SolveCpModel(model, &solver_model);
    if (!events.Close()) return kExitOutput;
    if (callback_failed.load()) {
      const protocol::WorkerFrame error = WorkerFailure(
          request->request_id(), protocol::WORKER_ERROR_CODE_INTERNAL,
          "callback event construction failed");
      return WriteSessionFrame(output, error, &output_bytes) ? 0 : kExitOutput;
    }
    bool invalid_assumption_core = false;
    const protocol::WorkerFrame finished = MakeFinished(
        *request, model, applied, applied_hash, response, &assumption_profile,
        &invalid_assumption_core);
    if (invalid_assumption_core) {
      const protocol::WorkerFrame error = WorkerFailure(
          request->request_id(), protocol::WORKER_ERROR_CODE_INTERNAL,
          "native assumption core failed whole-core validation");
      return WriteSessionFrame(output, error, &output_bytes) ? 0 : kExitOutput;
    }
    return WriteSessionFrame(output, finished, &output_bytes) ? 0 : kExitOutput;
  } catch (const std::bad_alloc&) {
    if (started_written) {
      try {
        const auto error = WorkerFailure(
            terminal_request_id, protocol::WORKER_ERROR_CODE_INTERNAL,
            "worker resource exhaustion prevented solve completion", true);
        if (WriteSessionFrame(output, error, &output_bytes)) return 0;
      } catch (...) {
      }
    }
    return kExitTemporary;
  } catch (...) {
    if (started_written) {
      try {
        const auto error = WorkerFailure(
            terminal_request_id, protocol::WORKER_ERROR_CODE_INTERNAL,
            "worker internal failure prevented solve completion");
        if (WriteSessionFrame(output, error, &output_bytes)) return 0;
      } catch (...) {
      }
    }
    return kExitInternal;
  }
}

}  // namespace eutheto::ortools_worker
