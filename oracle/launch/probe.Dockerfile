# The supervisor-side probe for oracle/launch: the pinned Alpine base plus Python.
ARG BASE_IMAGE
FROM ${BASE_IMAGE}
RUN apk add --no-cache python3
