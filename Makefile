.POSIX:

setup:
	sh scripts/setup-dev

check:
	sh scripts/check

lint:
	shellcheck -s sh scripts/setup-dev scripts/check scripts/artifact scripts/artifact-selftest

artifact-selftest:
	sh scripts/artifact-selftest

test: check

.PHONY: setup check lint test artifact-selftest
