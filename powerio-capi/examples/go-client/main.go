// A small Go client for PowerIO C ABI 7.
package main

/*
#include <stdlib.h>
#include "powerio.h"
*/
import "C"

import (
	"fmt"
	"os"
	"unsafe"
)

func text(view C.PioStringView) string {
	if view.data == nil || view.len == 0 {
		return ""
	}
	return C.GoStringN(view.data, C.int(view.len))
}

func fail(operation string, err *C.PioError) {
	if err != nil {
		fmt.Fprintf(os.Stderr, "%s: %s: %s\n", operation,
			text(C.pio_error_code(err)), text(C.pio_error_message(err)))
		C.pio_error_release(err)
	} else {
		fmt.Fprintln(os.Stderr, operation)
	}
	os.Exit(1)
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: go-client case9.m")
		os.Exit(2)
	}
	if C.pio_abi_version() != C.PIO_ABI_VERSION || C.PIO_ABI_VERSION != 7 {
		fail("ABI mismatch", nil)
	}

	path := C.CString(os.Args[1])
	defer C.free(unsafe.Pointer(path))
	var cerr *C.PioError
	source := C.pio_source_open(path, C.size_t(len(os.Args[1])), &cerr)
	if source == nil {
		fail("source", cerr)
	}
	module := C.pio_parse(source, nil, 0, &cerr)
	C.pio_source_release(source)
	if module == nil {
		fail("parse", cerr)
	}

	value := C.pio_module_value(module)
	if value == nil || text(C.pio_value_type_name(value)) != "powerio.BalancedNetwork" {
		fail("value type", nil)
	}
	network := C.pio_value_balanced_network(value, &cerr)
	C.pio_value_release(value)
	if network == nil {
		fail("balanced network", cerr)
	}
	C.pio_module_release(module)

	if C.pio_balanced_network_bus_count(network) != 9 {
		fail("unexpected bus count", nil)
	}
	incidence := C.pio_calc_incidence_matrix(network, nil, 0, &cerr)
	if incidence == nil {
		fail("incidence matrix", cerr)
	}
	if C.pio_sparse_matrix_rows(incidence) != C.pio_balanced_network_branch_count(network) {
		fail("incidence row count", nil)
	}
	C.pio_sparse_matrix_release(incidence)
	C.pio_balanced_network_release(network)

	fmt.Println("PowerIO Go ABI 7 probe passed")
}
