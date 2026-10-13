#!/bin/sh
read -r json
echo "Discovered: $SLICC_DISCOVER_STANDARD"
echo "Origin:     $SLICC_DISCOVER_ORIGIN"
echo "URL:        $SLICC_DISCOVER_URL"
echo "$json" | jq .
