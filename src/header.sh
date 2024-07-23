#!/bin/bash

# Self-extracting executable header

# Function to clean up temporary files
cleanup() {
    rm -rf "$TEMP_DIR"
}

# Set up trap to clean up on exit
trap cleanup EXIT

# Create a temporary directory
TEMP_DIR=$(mktemp -d)

# Extract the tarball to the temporary directory
tail -n +$((LINENO + 2)) "$0" | tar xz -C "$TEMP_DIR"

# Change to the temporary directory
cd "$TEMP_DIR" || exit 1

# Execute the main script or binary (adjust as needed)
./main

# Exit with the status of the main script
exit $?

# The tarball content will be appended below this line
