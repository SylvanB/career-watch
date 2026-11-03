//! Normalises raw job title / department / location strings into consistent
//! categorical buckets used for metric rollups.

// ── Department ────────────────────────────────────────────────────────────────

/// Normalised department categories.
pub const DEPT_ENGINEERING: &str = "engineering";
pub const DEPT_PRODUCT: &str = "product";
pub const DEPT_SALES: &str = "sales";
pub const DEPT_MARKETING: &str = "marketing";
pub const DEPT_DESIGN: &str = "design";
pub const DEPT_DATA: &str = "data";
pub const DEPT_OPERATIONS: &str = "operations";
pub const DEPT_PEOPLE: &str = "people";
pub const DEPT_FINANCE: &str = "finance";
pub const DEPT_LEGAL: &str = "legal";
pub const DEPT_CUSTOMER_SUCCESS: &str = "customer_success";
pub const DEPT_OTHER: &str = "other";

/// All known category slugs, used for snapshot column mapping.
pub const ALL_DEPT_CATEGORIES: &[&str] = &[
    DEPT_ENGINEERING,
    DEPT_PRODUCT,
    DEPT_SALES,
    DEPT_MARKETING,
    DEPT_DESIGN,
    DEPT_DATA,
    DEPT_OPERATIONS,
    DEPT_PEOPLE,
    DEPT_FINANCE,
    DEPT_LEGAL,
    DEPT_CUSTOMER_SUCCESS,
    DEPT_OTHER,
];

// Ordered: check more-specific patterns first so "data engineer" → data
// before falling through to engineering.
const DEPT_RULES: &[(&str, &[&str])] = &[
    (
        DEPT_DATA,
        &[
            "data scientist",
            "data science",
            "data engineer",
            "ml engineer",
            "machine learning engineer",
            "ai researcher",
            "research scientist",
            "data analyst",
            "business analyst",
            "analytics engineer",
            "business intelligence",
            "bi engineer",
            "data platform",
            "quantitative researcher",
            "quant researcher",
            "nlp engineer",
        ],
    ),
    (
        DEPT_ENGINEERING,
        &[
            "software engineer",
            "software developer",
            "swe ",
            "swe,",
            "swe/",
            "backend engineer",
            "frontend engineer",
            "full stack",
            "fullstack",
            "mobile engineer",
            "ios engineer",
            "android engineer",
            "infrastructure engineer",
            "platform engineer",
            "devops engineer",
            "site reliability",
            "sre ",
            "sre,",
            "security engineer",
            "systems engineer",
            "embedded engineer",
            "firmware engineer",
            "cloud engineer",
            "qa engineer",
            "test engineer",
            "quality engineer",
            "staff engineer",
            "principal engineer",
            "senior engineer",
            "junior engineer",
            "engineering manager",
            "head of engineering",
            "vp engineering",
            "cto",
            "solutions engineer",
            "solutions architect",
            "implementation engineer",
            "developer advocate",
            "developer relations",
            "devrel",
            "machine learning infra",
            "ml infra",
            "mlops",
        ],
    ),
    (
        DEPT_PRODUCT,
        &[
            "product manager",
            "product lead",
            "product owner",
            "product ops",
            "head of product",
            "vp product",
            "chief product",
            "cpo",
            "program manager",
            "technical program manager",
            "tpm",
            "product analyst",
            "product operations",
        ],
    ),
    (
        DEPT_DESIGN,
        &[
            "product designer",
            "ux designer",
            "ui designer",
            "ux researcher",
            "visual designer",
            "motion designer",
            "brand designer",
            "graphic designer",
            "interaction designer",
            "design manager",
            "head of design",
            "vp design",
            "creative director",
            "art director",
        ],
    ),
    (
        DEPT_SALES,
        &[
            "account executive",
            "account manager",
            "sales development",
            "sdr",
            "business development",
            "sales manager",
            "sales engineer",
            "revenue operations",
            "revops",
            "partnerships manager",
            "channel sales",
            "enterprise sales",
            "inside sales",
            "outbound sales",
            "head of sales",
            "vp sales",
            "chief revenue",
            "cro ",
        ],
    ),
    (
        DEPT_MARKETING,
        &[
            "marketing manager",
            "growth manager",
            "demand generation",
            "demand gen",
            "content marketer",
            "content strategist",
            "seo specialist",
            "paid media",
            "performance marketing",
            "brand manager",
            "communications manager",
            "public relations",
            "pr manager",
            "social media",
            "email marketing",
            "product marketing",
            "field marketing",
            "event marketing",
            "head of marketing",
            "vp marketing",
            "chief marketing",
            "cmo",
        ],
    ),
    (
        DEPT_CUSTOMER_SUCCESS,
        &[
            "customer success",
            "csm",
            "customer support",
            "technical support",
            "support engineer",
            "customer experience",
            "onboarding specialist",
            "implementation manager",
            "professional services",
            "head of customer",
            "vp customer",
        ],
    ),
    (
        DEPT_PEOPLE,
        &[
            "recruiter",
            "recruiting",
            "talent acquisition",
            "technical recruiter",
            "hr manager",
            "human resources",
            "people ops",
            "people operations",
            "hrbp",
            "hr business partner",
            "compensation",
            "total rewards",
            "learning and development",
            "l&d",
            "diversity",
            "dei",
            "head of people",
            "vp people",
            "chief people",
            "cpo",
        ],
    ),
    (
        DEPT_FINANCE,
        &[
            "finance manager",
            "financial analyst",
            "fp&a",
            "accounting",
            "controller",
            "treasurer",
            "head of finance",
            "vp finance",
            "chief financial",
            "cfo",
            "tax",
            "payroll",
        ],
    ),
    (
        DEPT_LEGAL,
        &[
            "legal counsel",
            "attorney",
            "general counsel",
            "associate counsel",
            "compliance manager",
            "privacy",
            "paralegal",
            "head of legal",
            "vp legal",
            "chief legal",
        ],
    ),
    (
        DEPT_OPERATIONS,
        &[
            "operations manager",
            "business operations",
            "strategy",
            "chief of staff",
            "executive assistant",
            "administrative",
            "facilities manager",
            "office manager",
            "it manager",
            "information technology",
            "information security",
            "infosec",
            "head of operations",
            "vp operations",
            "chief operating",
            "coo",
        ],
    ),
];

/// Classify a job title into a department category.
///
/// Checks the ordered `DEPT_RULES` list and returns the first matching
/// category. Falls back to `DEPT_OTHER` when nothing matches.
pub fn categorize_department(title: &str) -> &'static str {
    let lower = title.to_lowercase();
    for (category, keywords) in DEPT_RULES {
        if keywords.iter().any(|kw| lower.contains(kw)) {
            return category;
        }
    }
    DEPT_OTHER
}

// ── Geography ─────────────────────────────────────────────────────────────────

pub const GEO_REMOTE: &str = "remote";
pub const GEO_US: &str = "us";
pub const GEO_EUROPE: &str = "europe";
pub const GEO_APAC: &str = "apac";
pub const GEO_LATAM: &str = "latam";
pub const GEO_UNKNOWN: &str = "unknown";

pub const ALL_GEO_REGIONS: &[&str] = &[
    GEO_REMOTE,
    GEO_US,
    GEO_EUROPE,
    GEO_APAC,
    GEO_LATAM,
    GEO_UNKNOWN,
];

const REMOTE_SIGNALS: &[&str] = &[
    "remote",
    "anywhere",
    "distributed",
    "work from home",
    "wfh",
    "worldwide",
    "fully remote",
    "global",
    "virtual",
];

const US_SIGNALS: &[&str] = &[
    "united states",
    "usa",
    "u.s.",
    "san francisco",
    "bay area",
    "silicon valley",
    "new york",
    "nyc",
    "los angeles",
    "seattle",
    "austin",
    "chicago",
    "boston",
    "denver",
    "washington dc",
    "atlanta",
    "miami",
    "raleigh",
    "portland",
    "san diego",
    "phoenix",
    "dallas",
    "houston",
    "minneapolis",
    "detroit",
    "philadelphia",
    "baltimore",
    "pittsburgh",
    "st. louis",
    "kansas city",
    // state abbreviations when standalone (not part of another word)
    " ca ",
    " ny ",
    " wa ",
    " tx ",
    " co ",
    " il ",
    " ma ",
    " ga ",
    " fl ",
];

const EUROPE_SIGNALS: &[&str] = &[
    "london",
    "berlin",
    "amsterdam",
    "paris",
    "dublin",
    "stockholm",
    "copenhagen",
    "madrid",
    "barcelona",
    "lisbon",
    "zurich",
    "munich",
    "frankfurt",
    "vienna",
    "brussels",
    "oslo",
    "helsinki",
    "warsaw",
    "prague",
    "rome",
    "milan",
    "tel aviv",
    "united kingdom",
    "uk",
    "germany",
    "netherlands",
    "france",
    "ireland",
    "sweden",
    "denmark",
    "spain",
    "portugal",
    "switzerland",
    "austria",
    "belgium",
    "norway",
    "finland",
    "poland",
    "europe",
    "emea",
];

const APAC_SIGNALS: &[&str] = &[
    "singapore",
    "tokyo",
    "japan",
    "sydney",
    "melbourne",
    "australia",
    "hong kong",
    "beijing",
    "shanghai",
    "shenzhen",
    "india",
    "bangalore",
    "bengaluru",
    "mumbai",
    "delhi",
    "hyderabad",
    "pune",
    "seoul",
    "korea",
    "taipei",
    "taiwan",
    "jakarta",
    "indonesia",
    "malaysia",
    "kuala lumpur",
    "bangkok",
    "thailand",
    "new zealand",
    "apac",
    "asia pacific",
];

const LATAM_SIGNALS: &[&str] = &[
    "brazil",
    "são paulo",
    "sao paulo",
    "rio de janeiro",
    "mexico",
    "mexico city",
    "colombia",
    "bogota",
    "argentina",
    "buenos aires",
    "chile",
    "santiago",
    "peru",
    "lima",
    "latam",
    "latin america",
];

/// Normalise a raw location string into `(region, is_remote)`.
///
/// Returns `(GEO_UNKNOWN, false)` when the location is blank or unrecognised.
pub fn categorize_location(raw: Option<&str>) -> (&'static str, bool) {
    let loc = match raw {
        Some(s) if !s.trim().is_empty() => s.to_lowercase(),
        _ => return (GEO_UNKNOWN, false),
    };

    // Remote takes priority — "Remote, US" is still primarily remote.
    if REMOTE_SIGNALS.iter().any(|s| loc.contains(s)) {
        return (GEO_REMOTE, true);
    }

    for (signals, region) in [
        (US_SIGNALS as &[&str], GEO_US),
        (EUROPE_SIGNALS, GEO_EUROPE),
        (APAC_SIGNALS, GEO_APAC),
        (LATAM_SIGNALS, GEO_LATAM),
    ] {
        if signals.iter().any(|s| loc.contains(s)) {
            return (region, false);
        }
    }

    (GEO_UNKNOWN, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Department ────────────────────────────────────────────────────────────

    #[test]
    fn classifies_engineering_roles() {
        assert_eq!(
            categorize_department("Senior Software Engineer"),
            DEPT_ENGINEERING
        );
        assert_eq!(
            categorize_department("Staff Backend Engineer"),
            DEPT_ENGINEERING
        );
        assert_eq!(
            categorize_department("Site Reliability Engineer"),
            DEPT_ENGINEERING
        );
        assert_eq!(categorize_department("iOS Engineer"), DEPT_ENGINEERING);
    }

    #[test]
    fn data_engineer_goes_to_data_not_engineering() {
        assert_eq!(categorize_department("Data Engineer"), DEPT_DATA);
        assert_eq!(categorize_department("ML Engineer"), DEPT_DATA);
        assert_eq!(categorize_department("Senior Data Scientist"), DEPT_DATA);
    }

    #[test]
    fn classifies_product_roles() {
        assert_eq!(
            categorize_department("Senior Product Manager"),
            DEPT_PRODUCT
        );
        assert_eq!(
            categorize_department("Technical Program Manager"),
            DEPT_PRODUCT
        );
    }

    #[test]
    fn classifies_design_roles() {
        assert_eq!(categorize_department("Product Designer"), DEPT_DESIGN);
        assert_eq!(categorize_department("UX Researcher"), DEPT_DESIGN);
    }

    #[test]
    fn classifies_sales_roles() {
        assert_eq!(
            categorize_department("Account Executive, Enterprise"),
            DEPT_SALES
        );
        assert_eq!(
            categorize_department("Sales Development Representative"),
            DEPT_SALES
        );
    }

    #[test]
    fn classifies_people_ops() {
        assert_eq!(categorize_department("Technical Recruiter"), DEPT_PEOPLE);
        assert_eq!(
            categorize_department("People Operations Manager"),
            DEPT_PEOPLE
        );
    }

    #[test]
    fn unknown_title_falls_back_to_other() {
        assert_eq!(categorize_department("Chief Happiness Officer"), DEPT_OTHER);
        assert_eq!(categorize_department(""), DEPT_OTHER);
    }

    // ── Geography ─────────────────────────────────────────────────────────────

    #[test]
    fn remote_signals_return_is_remote_true() {
        assert_eq!(categorize_location(Some("Remote")), (GEO_REMOTE, true));
        assert_eq!(
            categorize_location(Some("Anywhere / Remote")),
            (GEO_REMOTE, true)
        );
        assert_eq!(categorize_location(Some("Remote, US")), (GEO_REMOTE, true));
    }

    #[test]
    fn classifies_us_cities() {
        assert_eq!(
            categorize_location(Some("San Francisco, CA")),
            (GEO_US, false)
        );
        assert_eq!(categorize_location(Some("New York, NY")), (GEO_US, false));
        assert_eq!(categorize_location(Some("Seattle, WA")), (GEO_US, false));
    }

    #[test]
    fn classifies_european_cities() {
        assert_eq!(categorize_location(Some("London, UK")), (GEO_EUROPE, false));
        assert_eq!(
            categorize_location(Some("Berlin, Germany")),
            (GEO_EUROPE, false)
        );
        assert_eq!(categorize_location(Some("Amsterdam")), (GEO_EUROPE, false));
    }

    #[test]
    fn classifies_apac() {
        assert_eq!(categorize_location(Some("Singapore")), (GEO_APAC, false));
        assert_eq!(
            categorize_location(Some("Bangalore, India")),
            (GEO_APAC, false)
        );
    }

    #[test]
    fn unknown_location_returns_unknown() {
        assert_eq!(categorize_location(None), (GEO_UNKNOWN, false));
        assert_eq!(categorize_location(Some("")), (GEO_UNKNOWN, false));
        assert_eq!(categorize_location(Some("Mars")), (GEO_UNKNOWN, false));
    }
}
