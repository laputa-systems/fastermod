helper() {
  local y=$1
  echo "$y"
}
count=1
helper 5
if [ $count -gt 0 ]; then
  inner=2
  echo $inner
fi
for i in 1 2 3; do
  echo $i
done
