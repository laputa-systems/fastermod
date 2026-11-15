package main

func helper(x int) int { return x }

func main() {
	count := helper(1, 2)
	_ = count
	other := helper(x)
	_ = other
	// helper(9) in comment
}
