An agent credential is what a session's agent signs in to the model with. It is one of your [secrets](help:secrets), stored under a name the agent CLI expects. You add it through the guided **Add agent credential** form on the [Secrets](/secrets) page, so you never type that name and never add it to a profile. Every session receives exactly one credential, without its profile asking for it.

### The two kinds

| Kind | Where it comes from | Billing |
| --- | --- | --- |
| Claude subscription token | Printed by `claude setup-token` | Needs a Pro or Max subscription |
| Anthropic API key | Created in the Anthropic Console | Billed per use |

### Who it applies to

**Applies to** decides the scope the credential is stored at:

- **Me**: sessions you launch.
- **A project**: sessions of that one project, whoever launches them.
- **Everyone**: sessions of every project.

Each scope holds one credential at most. To switch a scope from one kind to the other, replace or delete the credential already there first. The form refuses a second one and names the credential that is in the way.

### Which one a session uses

The most specific credential that applies wins, whatever its kind:

1. the launching person's own (**Me**);
2. the project's;
3. **Everyone**'s.

Say you keep a subscription token for yourself and the project has an API key. Your launches then use your token, and a teammate with no credential of their own uses the project's key. The launch form and the profile editor both tell you which credential a launch would use, before anything starts. That answer is for you, and a teammate opening the same page sees their own.

With no credential at all, the launch form offers **Add credential** first. **Launch anyway** is still there, because a session image that carries its own sign-in needs none. A normal session launched without one fails to authenticate.

### Automatic and scheduled runs

Sessions started by the dispatcher or by a schedule have no person behind them, so a **Me** credential never reaches them. They need a credential stored for the project or for **Everyone**. A profile that has neither can't be saved with automatic launching or a schedule turned on. If that credential is later deleted, the runs are skipped until one is back. [Automation](help:automation) has the details.

### When a credential stops working

Subscription tokens expire, and keys get revoked. When the agent can't authenticate, the session shows an error naming the secret it was given and where that secret is stored, and a conversational session parks. Replace that credential's value on the Secrets page, then send the session your next message. It resumes with the new value, and nothing else needs restarting.
