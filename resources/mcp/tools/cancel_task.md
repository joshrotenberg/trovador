Cancel a running task and wait up to 55 seconds for provider process cleanup.
cancelled means cleanup returned; cancelling means cleanup is still in progress
and should be checked with wait_tasks. Calling on a terminal task returns its
unchanged result.
