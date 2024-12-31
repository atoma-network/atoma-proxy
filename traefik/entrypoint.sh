 #!/bin/sh
set -e

# Create acme.json if it doesn't exist
touch /acme.json

# Set correct permissions (600) for acme.json
chmod 600 /acme.json

# Start Traefik
exec "$@"