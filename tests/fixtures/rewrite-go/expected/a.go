package main

func helper(x int) int { return x }

func main() {
	count := renamed(1, 2)
	_ = count
	other := renamed(x)
	_ = other
	// helper(9) in comment
}
