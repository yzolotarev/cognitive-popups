# MRT verified-error / version 1

Approved explicitly by the author in the implementation session on 2026-10-27.

- Eligibility: actual code-checked independent error with a frozen criterion and
  explicitly bound unit; reliable current task context; no interfering task/window.
- Default off. Separate opt-in for outside-session suggestions. Recheck consent
  before delivery; revocation cancels pending delivery.
- Assignment: independent OS-random draw, probability 0.5 short diagnostic
  suggestion, 0.5 `do_nothing`. Both count toward frequency limits.
- Frequency: at least 900 seconds between assignments; at most 3 per UTC day.
- Delivery: system notification only. Chosen, delivered, failed and revoked are
  distinct states. Control is not a refusal and has no fabricated delivery.
- Outcome: a different, independent code-checked task on the same unit within
  86400 seconds after assignment, equally observable in both arms. No check by
  the end of that window becomes `missing`, never an incorrect answer.
- Clicks and time in the UI are not learning outcomes and never create reviews.
- Report: assignment/delivery/observed/pending/missing counts and independent
  check accuracy with Wilson 95% intervals per arm. This is descriptive only;
  repeated observations on one person are not independent population samples.
  No causal efficacy claim, automatic policy update or proven optimization.
- The suggestion offers an optional independent diagnostic; it is not a
  mandatory quiz and does not steal focus from external work.
