#include "cpg/cancellation.h"
#include "cpg/error.h"
#include "cpg/win_handle.h"

namespace cpg {

struct Cancellation::State {
    std::atomic_bool flag{false};
    UniqueHandle event;
};

Cancellation::Cancellation() : state_(std::make_shared<State>())
{
    state_->event.reset(CreateEventW(nullptr, TRUE, FALSE, nullptr));
}

void Cancellation::cancel() const noexcept
{
    state_->flag.store(true, std::memory_order_release);
    if (state_->event.valid()) SetEvent(state_->event.get());
}

bool Cancellation::cancelled() const noexcept
{
    return state_->flag.load(std::memory_order_acquire);
}

void Cancellation::check() const
{
    if (cancelled()) throw Error("LAUNCH_CANCELLED", "Operation cancelled.");
}

HANDLE Cancellation::eventHandle() const noexcept
{
    return state_->event.get();
}

} // namespace cpg
