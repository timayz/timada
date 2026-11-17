# A product's page: its price, the options to pick, its facts, what shoppers
# said of it and what goes with it.

product_rating =
    { $count ->
        [one] { NUMBER($rating, minimumFractionDigits: 1, maximumFractionDigits: 1) }/5 · { $count } review
       *[other] { NUMBER($rating, minimumFractionDigits: 1, maximumFractionDigits: 1) }/5 · { $count } reviews
    }
product_sold_out = Sold out
product_few_left = Only { $left } left in stock
product_quantity = Quantity
    .label = quantity of { $name }
product_delivery_within = Tracked delivery in { $days }
product_delivery = Tracked delivery
product_money_back = 14-day money-back guarantee
product_service = Customer service here to help
product_variants = Prices by option
product_variants_price = Price
product_variants_stock = Stock
product_variant_few_left = Only { $left } left
product_variant_in_stock = In stock
product_description = Description
product_facts = Additional information
product_reviews = Customer reviews
    .pages = Review pages
product_reviews_none = No reviews yet. Bought it? Leave your review from your order's page.
product_review_photo = A photo from the customer
product_review_reply = Reply from the shop
product_reviews_newer = Newer reviews
product_reviews_page = Page { $page } of { $pages }
product_reviews_older = Older reviews
product_related = Similar products
