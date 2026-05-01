# anthropic_stream_aborted

This fixture has no event / body — aborted streams produce no terminal
event by definition. The matrix test constructs the expected
`UsageOutcome::Error` directly to confirm `ErrorClass::StreamAborted` is
the contracted classification.
