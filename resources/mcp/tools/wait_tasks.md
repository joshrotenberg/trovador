Collect task results. Returns when any requested task is terminal or the bounded
timeout expires. timeout_ms=0 reads status immediately. Remove completed IDs before
waiting again. Does not cancel workers when waiting ends.
Each snapshot includes the original effort and selection metadata: requested
versus selected settings, with reported model/effort null because actual per-turn
settings cannot be verified.
