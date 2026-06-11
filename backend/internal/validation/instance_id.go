package validation

import (
	"fmt"
	"regexp"
	"strings"
)

const MaxInstanceIDLen = 64

var instanceIDPattern = regexp.MustCompile(`^[a-z][a-z0-9_-]*$`)

func ValidateInstanceID(id string) error {
	id = strings.TrimSpace(id)
	if id == "" {
		return fmt.Errorf("id is required")
	}
	if len(id) > MaxInstanceIDLen {
		return fmt.Errorf("id must be at most %d characters", MaxInstanceIDLen)
	}
	if !instanceIDPattern.MatchString(id) {
		return fmt.Errorf("id must start with a lowercase letter and contain only lowercase letters, digits, hyphens, and underscores")
	}
	return nil
}
