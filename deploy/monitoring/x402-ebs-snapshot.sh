#!/bin/bash
# Take a daily EBS snapshot of the host's root volume and prune old automated
# snapshots. Pushes SnapshotSuccess{} to CloudWatch so a companion alarm with
# missing-data-as-breaching mirrors x402-backup-missing.
#
# PREREQUISITE: the instance role (x402-near-backup-role) needs an IAM policy
# allowing ec2:CreateSnapshot, ec2:CreateTags (on snapshots),
# ec2:DescribeSnapshots, and ec2:DeleteSnapshot before this timer is enabled;
# see deploy/monitoring/README.md. Until then the unit stays installed but
# disabled.
set -euo pipefail

readonly METRIC_REGION=us-east-1     # colocated with the alert topic + alarms
readonly EC2_REGION=us-west-2        # where the instance and volume live
readonly NAMESPACE=x402near
readonly VOLUME_ID=vol-09320dcb0d9aa4faf
readonly RETAIN_DAYS=14
readonly TAG_KEY=x402-automated-snapshot

snapshot_id=$(aws ec2 create-snapshot \
  --region "$EC2_REGION" \
  --volume-id "$VOLUME_ID" \
  --description "x402 host automated daily snapshot $(date -u +%Y-%m-%dT%H:%MZ)" \
  --tag-specifications "ResourceType=snapshot,Tags=[{Key=$TAG_KEY,Value=true},{Key=Name,Value=x402-host-daily}]" \
  --query SnapshotId --output text)
echo "created $snapshot_id"

# Prune automated snapshots older than the retention window. Only snapshots
# carrying our tag are candidates; manual snapshots are never touched.
cutoff=$(date -u -d "-$RETAIN_DAYS days" +%Y-%m-%dT%H:%M:%SZ)
aws ec2 describe-snapshots \
  --region "$EC2_REGION" \
  --owner-ids self \
  --filters "Name=tag:$TAG_KEY,Values=true" "Name=volume-id,Values=$VOLUME_ID" \
  --query "Snapshots[?StartTime<'$cutoff'].SnapshotId" --output text |
tr '\t' '\n' | while read -r old; do
  [ -n "$old" ] || continue
  aws ec2 delete-snapshot --region "$EC2_REGION" --snapshot-id "$old"
  echo "pruned $old"
done

aws cloudwatch put-metric-data \
  --region "$METRIC_REGION" \
  --namespace "$NAMESPACE" \
  --metric-name SnapshotSuccess \
  --value 1 \
  --unit None
