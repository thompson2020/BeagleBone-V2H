#!/bin/bash
set -e

REMOTE_USER="unit"
REMOTE_IP="192.168.10.100"
REMOTE_PATH="/home/unit/v2h_webui/svelte-skeleton-ui"
SERVICE="v2h_webui.service"

PROJECT_DIR="$HOME/v2h_webui/svelte-skeleton-ui"

echo "========================================"
echo "📤 Deploying V2H Web UI to Pi (${REMOTE_IP})"
echo "========================================"

if [ ! -f "$PROJECT_DIR/build/index.js" ]; then
    echo "❌ Build not found! Run build_v2h_webui.sh first."
    exit 1
fi

cd "$PROJECT_DIR"

echo "→ Stopping service..."
ssh $REMOTE_USER@$REMOTE_IP "sudo systemctl stop $SERVICE || true"

echo "→ Ensuring remote directory exists..."
ssh $REMOTE_USER@$REMOTE_IP "mkdir -p $REMOTE_PATH/build"

echo "→ Transferring build..."
rsync -avz --delete --progress \
    build/ \
    $REMOTE_USER@$REMOTE_IP:$REMOTE_PATH/build/

echo "→ Transferring server.js..."
rsync -avz --progress \
    server.js \
    $REMOTE_USER@$REMOTE_IP:$REMOTE_PATH/

echo "→ Restarting service..."
ssh $REMOTE_USER@$REMOTE_IP "
    sudo systemctl restart $SERVICE &&
    echo '=== Deployed at \$(date) ===' &&
    sudo systemctl status $SERVICE --no-pager -l | head -n 20
"

echo "✅ Deployment completed successfully!"

# DO THIS ON THE RASPBERRY PI
# Create a sudoers rule for unit
#sudo visudo -f /etc/sudoers.d/unit-deploy
#unit ALL=(ALL) NOPASSWD: /usr/bin/systemctl stop v2h_webui.service
#unit ALL=(ALL) NOPASSWD: /usr/bin/systemctl restart v2h_webui.service
#unit ALL=(ALL) NOPASSWD: /usr/bin/systemctl status v2h_webui.service

#sudo chmod 0440 /etc/sudoers.d/unit-deploy
