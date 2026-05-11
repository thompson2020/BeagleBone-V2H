#!/bin/bash
set -e

BINARY="indra_beaglebone"
REMOTE_USER="unit"
REMOTE_IP="192.168.10.101"
REMOTE_PATH="/home/unit/bin"
SERVICE="indra_beaglebone.service"

echo "========================================"
echo "📤 Deploying to BeagleBone (${REMOTE_IP})"
echo "========================================"

PROJECT_DIR="$HOME/BeagleBone-V2H"

if [ ! -f "$PROJECT_DIR/target/arm-unknown-linux-musleabihf/release/$BINARY" ]; then
    echo "❌ Binary not found! Run build first."
    exit 1
fi

cd "$PROJECT_DIR"

echo "→ Stopping service..."
ssh $REMOTE_USER@$REMOTE_IP "sudo systemctl stop $SERVICE"

echo "→ Transferring binary..."
rsync -vz --progress \
    target/arm-unknown-linux-musleabihf/release/$BINARY \
    $REMOTE_USER@$REMOTE_IP:$REMOTE_PATH/$BINARY

echo "→ Transferring config..."
if [ -f "$PROJECT_DIR/config.toml" ]; then
    rsync -vz "$PROJECT_DIR/config.toml" $REMOTE_USER@$REMOTE_IP:$REMOTE_PATH/config.toml
    echo "  config.toml transferred"
else
    echo "  ⚠️  No local config.toml found — BeagleBone keeps its existing config"
    echo "     (copy config.example.toml to config.toml and fill in credentials)"
fi

echo "→ Setting permissions and restarting..."
ssh $REMOTE_USER@$REMOTE_IP "
    sudo chown root:root $REMOTE_PATH/$BINARY &&
    sudo chmod +x $REMOTE_PATH/$BINARY &&
    sudo systemctl restart $SERVICE &&
    echo '=== Deployed at \$(date) ===' &&
    sudo systemctl status $SERVICE --no-pager -l | head -n 20
"

echo "✅ Deployment completed successfully!"

# DO THIS ON THE BEAGLEBONE
# Create a sudoers rule for unit
#sudo visudo -f /etc/sudoers.d/unit-deploy
#unit ALL=(ALL) NOPASSWD: /usr/bin/systemctl stop indra-beaglebone.service
#unit ALL=(ALL) NOPASSWD: /usr/bin/systemctl restart indra-beaglebone.service
#unit ALL=(ALL) NOPASSWD: /usr/bin/systemctl status indra-beaglebone.service
#unit ALL=(ALL) NOPASSWD: /bin/chown
#unit ALL=(ALL) NOPASSWD: /bin/chmod

#sudo chmod 0440 /etc/sudoers.d/unit-deploy
