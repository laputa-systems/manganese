.POSIX:

setup:
	sh scripts/setup-dev

check:
	sh scripts/check

lint:
	shellcheck -s sh scripts/setup-dev scripts/check scripts/artifact

test: check

.PHONY: setup check lint test
