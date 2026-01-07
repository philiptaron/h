# h

Faster shell navigation of projects.

## Usage

```bash
# Add to your shell profile
eval "$(h --setup ~/src)"

# Navigate to a repo (searches ~/src)
h myproject

# Clone and navigate to a GitHub repo
h user/repo

# Navigate up to project root
eval "$(up --setup)"
up
```

## Credit

Based on [zimbatm/h](https://github.com/zimbatm/h) by Jonas Chevalier.
