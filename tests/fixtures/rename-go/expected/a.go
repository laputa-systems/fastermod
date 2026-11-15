package main

func renamed(x int) int { return x }

func main() {
	count := renamed(1)
	helper := 5
	_ = helper
	_ = count
}
