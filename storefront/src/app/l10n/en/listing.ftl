# The listings: the menu, the whole catalogue, a category, the filters and
# the search box's dropdown.

listing_menu = Categories
listing_menu_close = Close the menu
listing_menu_all = All { $name }
listing_results_for = Results for “{ $q }”
listing_subcategories = Subcategories
listing_filters = Show filters
listing_filter_by_price = Filter by price
listing_price_band = From { $from } to { $to }
listing_filter_by = Filter by { $name }
listing_filters_clear = Clear filters
listing_results =
    { $count ->
        [one] { $count } result shown
       *[other] { $count } results shown
    }
listing_sort = Sort
listing_sort_best_selling = Sort by popularity
listing_sort_newest = Sort by newest
listing_sort_top_rated = Sort by average rating
listing_sort_price_asc = Sort by price: low to high
listing_sort_price_desc = Sort by price: high to low
listing_none_filtered = No product matches these filters.
listing_none = No product matches for now.
listing_category_none = Nothing in this category for now.
listing_pages = Pages
listing_previous = ← Previous
listing_page = Page { $page } of { $pages }
listing_next = Next →
listing_questions = Frequently asked questions
listing_search_none = No results for “{ $q }”
listing_search_category = Category
listing_search_all = See all results ({ $count })
listing_search_others = Other results
listing_search_page = Page
