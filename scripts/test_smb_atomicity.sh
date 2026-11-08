#!/usr/bin/env bash
set -euo pipefail

# This is a protocol gate, not a product upload. Only disposable localhost
# data is used; a server/client pair that cannot prove no-replace stays closed.
test_dir=$(mktemp -d /tmp/ricefield-smb-probe-XXXXXXXX)
server_pid=''
cleanup() {
  if [[ -n "$server_pid" ]]; then
    sudo kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf -- "$test_dir"
}
trap cleanup EXIT

mkdir -p "$test_dir"/{share,state,lock,cache,private,pid}
chmod 755 "$test_dir"
chmod 777 "$test_dir/share"
config="$test_dir/smb.conf"
cat > "$config" <<CONF
[global]
    server role = standalone server
    security = user
    map to guest = Bad User
    guest account = nobody
    interfaces = 127.0.0.1
    bind interfaces only = yes
    disable netbios = yes
    server min protocol = SMB2_10
    server max protocol = SMB3_11
    smb ports = 14445
    pid directory = $test_dir/pid
    lock directory = $test_dir/lock
    state directory = $test_dir/state
    cache directory = $test_dir/cache
    private dir = $test_dir/private
    log file = $test_dir/smb.log
    load printers = no
[disposable]
    path = $test_dir/share
    guest ok = yes
    guest only = yes
    read only = no
    force user = nobody
    create mask = 0666
    directory mask = 0777
CONF

sudo smbd --foreground --no-process-group --configfile="$config" --port=14445 --debuglevel=0 &
server_pid=$!

smb() {
  timeout 20s smbclient //127.0.0.1/disposable --port=14445 --no-pass \
    --user=__ricefield_test_guest__ --max-protocol=SMB3 \
    --option='client min protocol=SMB2_10' --command="$1" >/dev/null 2>&1
}

connected=0
for attempt in {1..30}; do
  if smb 'ls'; then
    connected=1
    break
  fi
  sleep 0.2
done
if [[ "$connected" != 1 ]]; then
  echo 'SMB test server not reachable; smbd log:' >&2
  tail -n 35 "$test_dir/smb.log" >&2 || true
  exit 1
fi

printf 'smb-snapshot-a\n' > "$test_dir/a"
printf 'smb-snapshot-b\n' > "$test_dir/b"
smb 'mkdir stage-a'
smb 'mkdir stage-b'
smb "put $test_dir/a stage-a/payload"
smb "put $test_dir/b stage-b/payload"
smb "get stage-a/payload $test_dir/download-a"
cmp "$test_dir/a" "$test_dir/download-a"

smb 'hardlink stage-a/payload winner'
cmp "$test_dir/a" "$test_dir/share/winner"

printf 'foreign-winner\n' > "$test_dir/share/occupied"
if smb 'hardlink stage-a/payload occupied'; then
  echo 'UNSAFE: SMB hardlink reported success for occupied destination' >&2
  exit 1
fi
printf 'foreign-winner\n' | cmp - "$test_dir/share/occupied"

smb 'hardlink stage-a/payload race' &
first=$!
smb 'hardlink stage-b/payload race' &
second=$!
left=0
right=0
wait "$first" || left=$?
wait "$second" || right=$?
if [[ "$left" == 0 && "$right" == 0 ]] || [[ "$left" != 0 && "$right" != 0 ]]; then
  echo "UNSAFE: two independent clients returned outcomes left=$left right=$right" >&2
  exit 1
fi
if [[ "$left" == 0 ]]; then
  cmp "$test_dir/a" "$test_dir/share/race"
else
  cmp "$test_dir/b" "$test_dir/share/race"
fi

echo 'SMB2/3 real-server no-replace hardlink protocol gate passed.'
