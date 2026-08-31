use std::path::Path;

pub fn git_commit_all(repo_root: &Path) {
    let repo = git2::Repository::init(repo_root).expect("git init");
    let mut index = repo.index().expect("open git fixture index");
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .expect("stage git fixture");
    index.write().expect("write git fixture index");
    let tree_id = index.write_tree().expect("write git fixture tree");
    let tree = repo.find_tree(tree_id).expect("load git fixture tree");
    let signature =
        git2::Signature::now("test", "test@example.com").expect("create git fixture signature");
    repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .expect("commit git fixture");
}
