#!/bin/bash

# Name of the environment variable to check
ENV_VAR_NAME="TRAMPOLINE_TEST_ENV"

# Expected value
EXPECTED_VALUE="teapot"

# Get the value of the environment variable
ACTUAL_VALUE=$(printenv "$ENV_VAR_NAME")

# Check if the environment variable is set
if [ -z "$ACTUAL_VALUE" ]; then
    echo "Error: Environment variable '$ENV_VAR_NAME' is not set."
    exit 1
fi

# Assert that the value matches the expected value
if [ "$ACTUAL_VALUE" == "$EXPECTED_VALUE" ]; then
    echo "Success: '$ENV_VAR_NAME' is set to the expected value."
else
    echo "Error: '$ENV_VAR_NAME' is set to '$ACTUAL_VALUE', but expected '$EXPECTED_VALUE'."
    exit 1
fi
