Cancel a running task and wait up to 55 seconds for provider process cleanup.
cancelled means cleanup returned; cancelling means cleanup is still in progress
and should be checked with wait_tasks. Calling on a terminal task returns its
unchanged result.

Only the owning server can cancel a live task. Another server returns an error
naming the owner. An interrupted task is already terminal.
