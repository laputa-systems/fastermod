package main

func renamed(x int) int { return x }

func main() {
	count := renamed(1)
	_ = count
}
